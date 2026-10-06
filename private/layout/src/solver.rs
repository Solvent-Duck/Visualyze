use crate::{
    projection::{Node, NodeKind, Projection},
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Algorithm {
    Layered,
    #[default]
    Floorplan,
    Constrained,
    Treemap,
}
#[derive(Debug, Clone, Copy)]
pub struct QualityProfile {
    pub clearance: i64,
    pub gap: i64,
    pub routed_candidates: usize,
    /// Additional routed placements generated from measured routing evidence.
    /// Zero disables feedback for controlled initializer comparisons.
    pub feedback_candidates: usize,
    /// Additional size-range placements, used only when ordinary placement and
    /// routing feedback found no feasible result. Also capped by work_budget.
    /// Zero disables this compatibility fallback for controlled comparisons.
    pub size_candidates: usize,
    pub movement_weight: f64,
}
impl Default for QualityProfile {
    fn default() -> Self {
        Self {
            clearance: 2,
            gap: 12,
            routed_candidates: 8,
            feedback_candidates: 4,
            size_candidates: 28,
            movement_weight: 0.08,
        }
    }
}
/// Budgets count candidate evaluations, never wall-clock time. The same input
/// and budget produce the same output independently of thread scheduling.
#[derive(Clone, Copy)]
pub struct LayoutRequest<'a> {
    pub projection: &'a Projection,
    pub algorithm: Algorithm,
    pub pins: &'a BTreeMap<String, Point>,
    pub previous: Option<&'a LayoutSolution>,
    pub quality: QualityProfile,
    pub work_budget: usize,
    pub cancelled: &'a AtomicBool,
}

/// The strict total spent by [`solve_budgeted`]. A candidate is charged before
/// construction/validation, including rejected or duplicate generated geometry.
/// Checking request metadata, deriving route evidence, and routing a charged
/// candidate do not charge it again. This is a deterministic candidate bound,
/// not a bound on elementary operations or elapsed time.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetedLayout {
    pub solution: LayoutSolution,
    pub evaluations: usize,
}

/// Preserve the established comparison-profile allocation: ordinary seeds,
/// feedback placements and feasibility size variants have separate allowances.
/// Call [`solve_budgeted`] when composing multiple solves under one total bound.
pub fn solve(req: LayoutRequest<'_>) -> Result<LayoutSolution, LayoutError> {
    solve_impl(req, &mut CandidateBudget::legacy(), &BTreeMap::new())
}

/// Solve with one strict candidate allowance across accepted-geometry rerouting,
/// initializers, adaptive feedback, and size-range fallback. An empty allowance
/// is invalid and is never rounded up. If adaptive stages are enabled, at most
/// ceil(remaining / 2) ordinary seeds are generated, preserving the remainder
/// for feedback and, only if still infeasible, legal dimension variants. Unspent
/// work is returned implicitly by `work_budget - evaluations`.
///
/// Existing accepted leaf sizes remain authoritative during repair; fresh size
/// variants retain the same allowed dimension domains as [`solve`]. This entry
/// point has a different allocation policy and does not change profile-5 `solve`.
pub fn solve_budgeted(req: LayoutRequest<'_>) -> Result<BudgetedLayout, LayoutError> {
    solve_budgeted_sizes(req, &BTreeMap::new())
}

/// Composed anchor envelopes are explicit hard constraints, not replacements
/// for the projection's allowed dimension domains or its normalization scale.
pub(crate) fn solve_budgeted_with_bounds(
    req: LayoutRequest<'_>,
    bounds: &BTreeMap<String, Bounds>,
) -> Result<BudgetedLayout, LayoutError> {
    let mut pins = req.pins.clone();
    let mut sizes = BTreeMap::new();
    for (id, bounds) in bounds {
        let node = req
            .projection
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .ok_or_else(|| LayoutError::Invalid(format!("unknown locked host {id}")))?;
        let size = bounds.size();
        if !node.solid
            || req
                .projection
                .nodes
                .iter()
                .any(|child| child.parent.as_ref() == Some(id))
            || [size.x, size.y, size.z].iter().any(|value| *value <= 0)
            || size.x < node.min_size.x
            || size.x > node.max_size.x
            || size.y < node.min_size.y
            || size.y > node.max_size.y
            || size.z < node.min_size.z
            || size.z > node.max_size.z
            || pins.get(id).is_some_and(|point| *point != bounds.center())
        {
            return Err(LayoutError::Invalid(format!(
                "locked host {id} conflicts with dimensions or pins"
            )));
        }
        pins.insert(id.clone(), bounds.center());
        sizes.insert(id.clone(), size);
    }
    let previous = req.previous.filter(|previous| {
        bounds.iter().all(|(id, bounds)| {
            previous
                .boxes
                .get(id)
                .is_some_and(|object| object.bounds == *bounds)
        })
    });
    let result = solve_budgeted_sizes(
        LayoutRequest {
            pins: &pins,
            previous,
            ..req
        },
        &sizes,
    )?;
    if bounds
        .iter()
        .any(|(id, bounds)| result.solution.boxes[id].bounds != *bounds)
    {
        return Err(LayoutError::Invalid(
            "candidate violates a locked host envelope".into(),
        ));
    }
    Ok(result)
}

fn solve_budgeted_sizes(
    req: LayoutRequest<'_>,
    fixed_sizes: &BTreeMap<String, Point>,
) -> Result<BudgetedLayout, LayoutError> {
    let mut budget = CandidateBudget::strict(req.work_budget);
    let mut solution = solve_impl(req, &mut budget, fixed_sizes)?;
    solution.diagnostics.push(format!(
        "Strict candidate budget: {} / {}; {} accepted-geometry trials, {} initializations, {} feedback placements, {} size-range placements (including rejected and duplicate generated geometry).",
        budget.evaluations, req.work_budget, budget.reused, budget.initializers,
        budget.feedback, budget.sizes,
    ));
    Ok(BudgetedLayout {
        solution,
        evaluations: budget.evaluations,
    })
}

#[derive(Clone, Copy)]
enum CandidateKind {
    Reused,
    Initializer,
    Feedback,
    Size,
}

struct CandidateBudget {
    limit: Option<usize>,
    evaluations: usize,
    reused: usize,
    initializers: usize,
    feedback: usize,
    sizes: usize,
}
impl CandidateBudget {
    fn legacy() -> Self {
        Self {
            limit: None,
            evaluations: 0,
            reused: 0,
            initializers: 0,
            feedback: 0,
            sizes: 0,
        }
    }
    fn strict(limit: usize) -> Self {
        Self {
            limit: Some(limit),
            ..Self::legacy()
        }
    }
    fn remaining(&self) -> usize {
        self.limit
            .map_or(usize::MAX, |limit| limit - self.evaluations)
    }
    fn take(&mut self, kind: CandidateKind) -> bool {
        if self.remaining() == 0 {
            return false;
        }
        // Legacy accounting is intentionally inactive: no changed allocation,
        // diagnostics, or overflow behavior in the existing solve entry point.
        if self.limit.is_some() {
            self.evaluations += 1;
            match kind {
                CandidateKind::Reused => self.reused += 1,
                CandidateKind::Initializer => self.initializers += 1,
                CandidateKind::Feedback => self.feedback += 1,
                CandidateKind::Size => self.sizes += 1,
            }
        }
        true
    }
    fn initializer_allowance(&self, req: &LayoutRequest<'_>) -> usize {
        if self.limit.is_none() {
            return req.work_budget.clamp(1, 512);
        }
        let available = self.remaining().min(req.work_budget);
        let count = if req.quality.feedback_candidates > 0 || req.quality.size_candidates > 0 {
            available.div_ceil(2)
        } else {
            available
        };
        count.min(512)
    }
}

fn solve_impl(
    req: LayoutRequest<'_>,
    budget: &mut CandidateBudget,
    fixed_sizes: &BTreeMap<String, Point>,
) -> Result<LayoutSolution, LayoutError> {
    check_cancel(req.cancelled)?;
    validate_request(&req)?;
    if let Some(previous) = req.previous {
        validate_previous(previous, req.projection)?;
    }
    let parents: BTreeSet<_> = req
        .projection
        .nodes
        .iter()
        .filter_map(|node| node.parent.as_deref())
        .collect();
    // Accepted, range-compatible leaf dimensions are authoritative during a
    // repair, including in the affected region. Resizing another leaf must not
    // restore a familiar neighbor to its original preferred dimensions.
    let mut preserved: BTreeMap<_, _> = req
        .projection
        .nodes
        .iter()
        .filter(|node| node.solid && !parents.contains(node.id.as_str()))
        .filter_map(|node| {
            req.previous
                .and_then(|previous| previous.boxes.get(&node.id))
                .filter(|old| node_compatible(node, old))
                .map(|old| (node.id.clone(), old.bounds.size()))
        })
        .collect();
    preserved.extend(fixed_sizes.iter().map(|(id, size)| (id.clone(), *size)));
    let ordinary_error = match solve_sizes(req, &preserved, 0, budget, CandidateKind::Initializer) {
        Ok(solution) => return Ok(solution),
        Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
        Err(error) => error,
    };
    let limit = req.quality.size_candidates.min(req.work_budget);
    let mut attempts = 0;
    let mut seen = BTreeSet::new();
    let mut best: Option<LayoutSolution> = None;
    for variant in 0..limit {
        check_cancel(req.cancelled)?;
        if budget.remaining() == 0 {
            break;
        }
        let mut sizes = preserved.clone();
        for node in &req.projection.nodes {
            if node.solid
                && !parents.contains(node.id.as_str())
                && !preserved.contains_key(&node.id)
            {
                sizes.insert(node.id.clone(), size_variant(node, variant));
            }
        }
        if sizes.iter().all(|(id, size)| {
            preserved.get(id).is_some_and(|old| old == size)
                || req
                    .projection
                    .nodes
                    .iter()
                    .find(|node| &node.id == id)
                    .is_some_and(|node| node.preferred_size == *size)
        }) || !seen.insert(
            sizes
                .iter()
                .map(|(id, size)| (id.clone(), *size))
                .collect::<Vec<_>>(),
        ) {
            continue;
        }
        attempts += 1;
        let candidate = solve_sizes(
            LayoutRequest {
                quality: QualityProfile {
                    routed_candidates: 1,
                    feedback_candidates: 0,
                    size_candidates: 0,
                    ..req.quality
                },
                work_budget: 1,
                ..req
            },
            &sizes,
            variant,
            budget,
            CandidateKind::Size,
        );
        match candidate {
            Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
            Err(_) => {}
            Ok(candidate) => {
                // This feasibility fallback does not reward uniformly tiny
                // boxes. Change dimensions as little as possible first; the
                // ordinary fixed-projection quality score breaks ties.
                let cost = |solution: &LayoutSolution| {
                    let change: i64 = req
                        .projection
                        .nodes
                        .iter()
                        .filter(|node| node.solid)
                        .map(|node| {
                            node.preferred_size
                                .manhattan(solution.boxes[&node.id].bounds.size())
                        })
                        .sum();
                    (
                        change,
                        solution.metrics.structural_cost
                            + req.quality.movement_weight * solution.metrics.displacement as f64,
                    )
                };
                if best.as_ref().is_none_or(|old| {
                    let (change, score) = cost(&candidate);
                    let (old_change, old_score) = cost(old);
                    change < old_change || (change == old_change && score < old_score)
                }) {
                    best = Some(candidate);
                }
            }
        }
    }
    check_cancel(req.cancelled)?;
    let work_note = if let Some(limit) = budget.limit {
        format!(
            "Size-range fallback: {attempts} size variants attempted within one strict total candidate budget; {} / {limit} evaluations used ({} initializations, {} accepted-geometry trials, {} feedback placements, {} size-range placements); accepted sizes and original quality scales preserved.",
            budget.evaluations, budget.initializers, budget.reused, budget.feedback, budget.sizes,
        )
    } else {
        format!(
            "Size-range fallback: {attempts} additional candidate evaluations (limit {limit}) after {} ordinary initializations and at most {} routing-feedback placements found no feasible result; accepted sizes and original quality scales preserved.",
            req.work_budget.clamp(1, 512),
            req.quality.feedback_candidates.min(req.work_budget)
        )
    };
    if let Some(mut solution) = best {
        solution.diagnostics.push(work_note);
        validate(req.projection, &solution, req.pins, req.quality.clearance)?;
        Ok(solution)
    } else if attempts > 0 {
        Err(match ordinary_error {
            LayoutError::Invalid(message) => {
                LayoutError::Invalid(format!("{message}; {work_note}"))
            }
            LayoutError::Unroutable(message) => {
                LayoutError::Unroutable(format!("{message}; {work_note}"))
            }
            LayoutError::Cancelled => LayoutError::Cancelled,
        })
    } else {
        Err(ordinary_error)
    }
}

/// Seven axis combinations, each at minimum, halfway toward minimum, halfway
/// toward maximum, and maximum. All choices are integer and input-order stable.
/// Ordinary objects do not acquire a more extreme aspect than their preferred
/// shape or 4:1; explicitly spanning concerns retain their slab thickness.
fn size_variant(node: &Node, variant: usize) -> Point {
    const MASKS: [u8; 7] = [7, 1, 2, 4, 3, 5, 6];
    let mask = MASKS[variant % MASKS.len()];
    let dimension = |axis: u8, minimum: i64, preferred: i64, maximum: i64| {
        if mask & axis == 0 || (node.kind == NodeKind::Concern && axis == 2) {
            return preferred;
        }
        match variant / MASKS.len() {
            0 => minimum,
            1 => minimum + (preferred - minimum) / 2,
            2 => preferred + (maximum - preferred) / 2,
            _ => maximum,
        }
    };
    let p = node.preferred_size;
    let lo = node.min_size;
    let hi = node.max_size;
    let size = Point::new(
        dimension(1, lo.x, p.x, hi.x),
        dimension(2, lo.y, p.y, hi.y),
        dimension(4, lo.z, p.z, hi.z),
    );
    let smallest = size.x.min(size.y).min(size.z);
    let largest = size.x.max(size.y).max(size.z);
    let preferred_min = p.x.min(p.y).min(p.z);
    let preferred_max = p.x.max(p.y).max(p.z);
    if node.kind != NodeKind::Concern
        && largest > smallest * 4
        && largest * preferred_min > preferred_max * smallest
    {
        p
    } else {
        size
    }
}

fn solve_sizes(
    req: LayoutRequest<'_>,
    size_overrides: &BTreeMap<String, Point>,
    seed_offset: usize,
    candidate_budget: &mut CandidateBudget,
    candidate_kind: CandidateKind,
) -> Result<LayoutSolution, LayoutError> {
    check_cancel(req.cancelled)?;
    let nodes: BTreeMap<_, _> = req
        .projection
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    if let Some(previous) = req.previous {
        validate_previous(previous, req.projection)?;
        // A new relation or revision does not itself justify moving anything.
        // Reuse legal geometry first, then route the current claims afresh.
        if compatible_geometry(&nodes, previous, req.pins, req.quality.gap)
            && candidate_budget.take(CandidateKind::Reused)
        {
            let mut reused = previous.clone();
            reused.revision = req.projection.revision;
            reused.projection_key = req.projection.key.clone();
            reused.diagnostics = req.projection.diagnostics.clone();
            for node in nodes.values() {
                reused.boxes.get_mut(&node.id).unwrap().semantic_ids = node.semantic_ids.clone();
            }
            if validation::validate_boxes(req.projection, &reused, req.pins).is_ok() {
                match routing::route_all(
                    &reused.boxes,
                    &req.projection.edges,
                    req.quality.clearance,
                    req.cancelled,
                ) {
                    Ok(routes) => {
                        reused.routes = routes;
                        validate(req.projection, &reused, req.pins, req.quality.clearance)?;
                        reused.metrics = measure(req.projection, &reused, req.previous);
                        check_cancel(req.cancelled)?;
                        return Ok(reused);
                    }
                    Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
                    Err(_) => {} // Attempt a bounded repair, still freezing unrelated regions.
                }
            }
        }
    }
    let affected = req.previous.map(|previous| {
        affected_regions(&nodes, req.projection, previous, req.pins, req.quality.gap)
    });
    let layered = (req.algorithm == Algorithm::Layered)
        .then(|| layered_ordering(&nodes, req.projection, req.work_budget, req.cancelled))
        .transpose()?;
    let mut seeds = Vec::new();
    let budget = candidate_budget.initializer_allowance(&req);
    for evaluation in 0..budget {
        let iteration = seed_offset + evaluation;
        check_cancel(req.cancelled)?;
        if !candidate_budget.take(candidate_kind) {
            break;
        }
        let mut boxes = BTreeMap::new();
        let mut roots: Vec<_> = nodes
            .values()
            .filter(|n| n.parent.is_none())
            .map(|n| n.id.clone())
            .collect();
        roots.sort();
        let mut offset = 0;
        for id in roots {
            pack_tree(
                &id,
                &nodes,
                req.projection,
                req.algorithm,
                iteration,
                req.quality.gap,
                size_overrides,
                layered.as_ref(),
                &mut boxes,
            )?;
            let bounds = boxes[&id].bounds;
            translate_tree(&id, Point::new(offset - bounds.min.x, 0, 0), &mut boxes);
            offset += bounds.size().x + req.quality.gap;
        }
        if let (Some(previous), Some(affected)) = (req.previous, &affected) {
            restore_unaffected(&nodes, &mut boxes, previous, affected, iteration);
        }
        let mut pins: Vec<_> = req.pins.iter().collect();
        pins.sort_by_key(|(id, _)| (node_depth(id, &nodes), *id));
        for (id, point) in pins {
            let shift = *point - boxes[id].bounds.center();
            translate_tree(id, shift, &mut boxes);
        }
        refit_frames(
            &nodes,
            &mut boxes,
            req.quality.gap / 2,
            req.pins,
            req.previous.is_some(),
        );
        let estimate = estimate(req.projection, &boxes);
        seeds.push((estimate, iteration, boxes));
    }
    seeds.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut best: Option<LayoutSolution> = None;
    let mut best_feedback = Vec::new();
    let mut failed_seed = None;
    let mut last_error = LayoutError::Invalid("no valid candidate".into());
    let mut seen = BTreeSet::new();
    let mut routed = 0;
    for (_, _, boxes) in seeds {
        check_cancel(req.cancelled)?;
        let fingerprint: Vec<_> = boxes
            .values()
            .map(|b| (&b.id, b.bounds.min, b.bounds.max))
            .map(|(id, a, b)| (id.clone(), a, b))
            .collect();
        if !seen.insert(fingerprint) {
            continue;
        }
        let mut solution = LayoutSolution {
            model_id: req.projection.model_id.clone(),
            revision: req.projection.revision,
            projection_key: req.projection.key.clone(),
            version: LAYOUT_VERSION,
            boxes,
            routes: vec![],
            metrics: Metrics::default(),
            diagnostics: req.projection.diagnostics.clone(),
        };
        if let Some(ordering) = &layered {
            solution.diagnostics.push(format!(
                "Layered initialization used {} alternating ordering sweeps and {} adjacent-neighbor comparisons (limit {}), then rectangular depth packing; supplied bands and relationship directions are unchanged.",
                ordering.sweeps, ordering.comparisons, ordering.comparison_limit,
            ));
        }
        if let Err(e) = validation::validate_boxes(req.projection, &solution, req.pins) {
            last_error = e;
            continue;
        }
        if !invalid_band_members(&nodes, &solution.boxes, req.quality.gap).is_empty() {
            last_error = LayoutError::Invalid(
                "pins or repair conflict with the selected responsibility layers".into(),
            );
            continue;
        }
        routed += 1;
        let mut feedback = Vec::new();
        match route_candidate(&solution, &req, &mut feedback) {
            Ok(routes) => solution.routes = routes,
            Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
            Err(e) => {
                last_error = e;
                if failed_seed.is_none() && !feedback.is_empty() {
                    failed_seed = Some((solution, feedback));
                }
                if routed >= req.quality.routed_candidates {
                    break;
                }
                continue;
            }
        }
        match validate(req.projection, &solution, req.pins, req.quality.clearance) {
            Ok(()) => {}
            Err(e) => {
                last_error = e;
                continue;
            }
        }
        solution.metrics = measure(req.projection, &solution, req.previous);
        let score = |s: &LayoutSolution| {
            s.metrics.structural_cost + req.quality.movement_weight * s.metrics.displacement as f64
        };
        if best.as_ref().is_none_or(|b| score(&solution) < score(b)) {
            best = Some(solution);
            best_feedback = feedback;
        }
        if routed >= req.quality.routed_candidates {
            break;
        }
    }
    let feedback_seed = best
        .as_ref()
        .map(|solution| (solution.clone(), best_feedback))
        .or(failed_seed);
    if let Some(seed) = feedback_seed {
        refine_routing_feedback(
            &req,
            &nodes,
            affected.as_ref(),
            seed,
            &mut best,
            &mut last_error,
            candidate_budget,
        )?;
    }
    check_cancel(req.cancelled)?;
    best.ok_or_else(|| if req.previous.is_some() {
        LayoutError::Invalid(format!("local repair cannot preserve unaffected assemblies: {last_error}; retain the accepted scene or request an explicit reorganization"))
    } else { last_error })
}

fn route_candidate(
    solution: &LayoutSolution,
    req: &LayoutRequest<'_>,
    feedback: &mut Vec<routing::RouteFeedback>,
) -> Result<Vec<Route>, LayoutError> {
    if req.quality.feedback_candidates == 0 {
        routing::route_all(
            &solution.boxes,
            &req.projection.edges,
            req.quality.clearance,
            req.cancelled,
        )
    } else {
        routing::route_all_with_feedback(
            &solution.boxes,
            &req.projection.edges,
            req.quality.clearance,
            req.cancelled,
            feedback,
        )
    }
}

type BoxFingerprint = Vec<(String, Point, Point)>;

fn geometry_key(solution: &LayoutSolution) -> BoxFingerprint {
    solution
        .boxes
        .values()
        .map(|b| (b.id.clone(), b.bounds.min, b.bounds.max))
        .collect()
}

/// A second, genuinely adaptive pass: open a corridor identified by an actual
/// routed detour or routing failure, then route again. Each improvement supplies
/// the evidence for the next move. Initializer permutations are not reused here.
fn refine_routing_feedback(
    req: &LayoutRequest<'_>,
    nodes: &BTreeMap<&str, &Node>,
    affected: Option<&BTreeSet<String>>,
    mut seed: (LayoutSolution, Vec<routing::RouteFeedback>),
    best: &mut Option<LayoutSolution>,
    last_error: &mut LayoutError,
    candidate_budget: &mut CandidateBudget,
) -> Result<(), LayoutError> {
    let budget = req.quality.feedback_candidates.min(req.work_budget);
    let mut seen = BTreeSet::from([geometry_key(&seed.0)]);
    let mut attempted = 0;
    let mut improvements = 0;
    let mut implicated = BTreeSet::new();
    while attempted < budget && !seed.1.is_empty() && candidate_budget.remaining() > 0 {
        check_cancel(req.cancelled)?;
        let moves = feedback_moves(
            &seed.0.boxes,
            &seed.1,
            req.pins,
            affected,
            req.quality.clearance,
        );
        let mut trial = None;
        for (id, shift, edge_id) in moves {
            check_cancel(req.cancelled)?;
            if !candidate_budget.take(CandidateKind::Feedback) {
                break;
            }
            let mut candidate = seed.0.clone();
            candidate.routes.clear();
            translate_tree(&id, shift, &mut candidate.boxes);
            refit_frames(
                nodes,
                &mut candidate.boxes,
                req.quality.gap / 2,
                req.pins,
                req.previous.is_some(),
            );
            if !seen.insert(geometry_key(&candidate)) {
                continue;
            }
            if validation::validate_boxes(req.projection, &candidate, req.pins).is_err()
                || !invalid_band_members(nodes, &candidate.boxes, req.quality.gap).is_empty()
            {
                continue;
            }
            trial = Some((candidate, edge_id));
            break;
        }
        let Some((mut candidate, edge_id)) = trial else {
            break;
        };
        attempted += 1;
        implicated.insert(edge_id);
        let mut feedback = Vec::new();
        match route_candidate(&candidate, req, &mut feedback) {
            Ok(routes) => candidate.routes = routes,
            Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
            Err(error) => {
                *last_error = error;
                // Before any feasible result, permit a failed corridor repair
                // to become the seed only when it measurably removes blockers.
                // This enables two-step passage opening without publishing it.
                if best.is_none()
                    && !feedback.is_empty()
                    && blocker_count(&feedback) < blocker_count(&seed.1)
                {
                    seed = (candidate, feedback);
                }
                continue;
            }
        }
        if let Err(error) = validate(req.projection, &candidate, req.pins, req.quality.clearance) {
            *last_error = error;
            continue;
        }
        candidate.metrics = measure(req.projection, &candidate, req.previous);
        let score = |solution: &LayoutSolution| {
            solution.metrics.structural_cost
                + req.quality.movement_weight * solution.metrics.displacement as f64
        };
        if best
            .as_ref()
            .is_none_or(|previous| score(&candidate) < score(previous))
        {
            improvements += 1;
            *best = Some(candidate.clone());
            seed = (candidate, feedback);
        }
    }
    if attempted > 0
        && let Some(best) = best
    {
        best.diagnostics.push(format!(
            "Routing feedback evaluated {attempted} new placements from {} measured detours/failures; retained {improvements} validated improvements (budget {budget}).",
            implicated.len(),
        ));
    }
    check_cancel(req.cancelled)
}

fn blocker_count(feedback: &[routing::RouteFeedback]) -> usize {
    feedback
        .iter()
        .flat_map(|feedback| feedback.blocked.iter().map(|block| &block.obstacle))
        .collect::<BTreeSet<_>>()
        .len()
}

fn feedback_moves(
    boxes: &BTreeMap<String, LayoutBox>,
    feedback: &[routing::RouteFeedback],
    pins: &BTreeMap<String, Point>,
    affected: Option<&BTreeSet<String>>,
    clearance: i64,
) -> Vec<(String, Point, String)> {
    let mut proposals = BTreeMap::new();
    for feedback in feedback {
        for blocked in &feedback.blocked {
            let mut id = blocked.obstacle.as_str();
            // An interface pad must move with its solid owner to retain its
            // attachment. Expanded frames and pinned subtrees are never moved.
            while let Some(parent) = boxes[id]
                .parent
                .as_deref()
                .filter(|parent| boxes[*parent].solid)
            {
                id = parent;
            }
            if !boxes[id].solid
                || pins.keys().any(|pin| is_descendant(pin, id, boxes))
                || affected.is_some_and(|regions| {
                    !regions
                        .iter()
                        .any(|region| is_descendant(id, region, boxes))
                })
            {
                continue;
            }
            let bounds = boxes[id].bounds;
            // Move horizontally only; responsibility layers retain their order.
            // Exactly touching a clearance envelope is a valid channel plane.
            for (dimension, start, end, low, high) in [
                (
                    0,
                    blocked.start.x,
                    blocked.end.x,
                    bounds.min.x,
                    bounds.max.x,
                ),
                (
                    2,
                    blocked.start.z,
                    blocked.end.z,
                    bounds.min.z,
                    bounds.max.z,
                ),
            ] {
                if start != end {
                    continue;
                }
                for delta in [start - clearance - high, start + clearance - low] {
                    let sign = if delta <= 0 { -1 } else { 1 };
                    let shift = if dimension == 0 {
                        Point::new(delta, 0, 0)
                    } else {
                        Point::new(0, 0, delta)
                    };
                    let key = (
                        !feedback.failed,
                        std::cmp::Reverse(feedback.excess_length),
                        shift.manhattan(Point::ZERO),
                        id.to_owned(),
                        dimension,
                        sign,
                    );
                    proposals.insert(key, (id.to_owned(), shift, feedback.edge_id.clone()));
                }
            }
        }
    }
    proposals.into_values().take(64).collect()
}

fn validate_request(req: &LayoutRequest<'_>) -> Result<(), LayoutError> {
    if req.quality.clearance < 1
        || req.quality.clearance > 100
        || req.quality.gap < req.quality.clearance * 4
        || req.quality.gap > 1000
        || req.quality.routed_candidates == 0
        || req.quality.feedback_candidates > 16
        || req.quality.size_candidates > 28
        || !req.quality.movement_weight.is_finite()
        || req.quality.movement_weight < 0.0
        || req.work_budget == 0
    {
        return Err(LayoutError::Invalid(
            "invalid quality profile or empty budget".into(),
        ));
    }
    if req.projection.nodes.is_empty() || req.projection.nodes.len() > 1024 {
        return Err(LayoutError::Invalid(
            "projection must contain 1–1024 visible objects".into(),
        ));
    }
    let nodes: BTreeMap<_, _> = req.projection.nodes.iter().map(|n| (&n.id, n)).collect();
    if nodes.len() != req.projection.nodes.len() {
        return Err(LayoutError::Invalid("duplicate display ID".into()));
    }
    for n in nodes.values() {
        for p in [n.min_size, n.max_size, n.preferred_size] {
            if p.x < 1 || p.y < 1 || p.z < 1 || p.x > 10000 || p.y > 10000 || p.z > 10000 {
                return Err(LayoutError::Invalid(format!(
                    "invalid dimensions: {}",
                    n.id
                )));
            }
        }
        if n.preferred_size.min(n.min_size) != n.min_size
            || n.preferred_size.max(n.max_size) != n.max_size
            || !(-64..=64).contains(&n.band)
        {
            return Err(LayoutError::Invalid(format!(
                "invalid size range or band: {}",
                n.id
            )));
        }
        let mut parent = n.parent.as_ref();
        let mut visited = BTreeSet::from([&n.id]);
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err(LayoutError::Invalid("cyclic display hierarchy".into()));
            }
            let Some(p) = nodes.get(id) else {
                return Err(LayoutError::Invalid(format!("missing parent {id}")));
            };
            parent = p.parent.as_ref();
        }
    }
    for (id, p) in req.pins {
        if !nodes.contains_key(id)
            || [p.x, p.y, p.z]
                .iter()
                .any(|c| !(-1_000_000..=1_000_000).contains(c))
        {
            return Err(LayoutError::Invalid(format!("invalid pin: {id}")));
        }
    }
    let mut edges = BTreeSet::new();
    if req.projection.edges.len() > 4096 {
        return Err(LayoutError::Invalid(
            "projection exceeds 4096 visible relationships".into(),
        ));
    }
    for e in &req.projection.edges {
        if !nodes.contains_key(&e.source)
            || !nodes.contains_key(&e.target)
            || e.source == e.target
            || !edges.insert(&e.id)
        {
            return Err(LayoutError::Invalid(format!("invalid edge {}", e.id)));
        }
    }
    Ok(())
}

fn validate_previous(
    previous: &LayoutSolution,
    projection: &Projection,
) -> Result<(), LayoutError> {
    if previous.model_id != projection.model_id
        || previous.version != LAYOUT_VERSION
        || previous.revision > projection.revision
    {
        return Err(LayoutError::Invalid(
            "previous layout belongs to another model, version, or a future revision".into(),
        ));
    }
    if previous.boxes.is_empty() || previous.boxes.len() > 1024 || previous.routes.len() > 4096 {
        return Err(LayoutError::Invalid(
            "previous layout exceeds bounded visible detail".into(),
        ));
    }
    for (id, object) in &previous.boxes {
        let b = object.bounds;
        if object.id != *id
            || [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]
                .iter()
                .any(|v| !(-10_000_000..=10_000_000).contains(v))
            || b.min.x >= b.max.x
            || b.min.y >= b.max.y
            || b.min.z >= b.max.z
        {
            return Err(LayoutError::Invalid(format!(
                "invalid previous bounds for {id}"
            )));
        }
        let mut seen = BTreeSet::new();
        let mut current = Some(id.as_str());
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(LayoutError::Invalid(
                    "cyclic previous layout hierarchy".into(),
                ));
            }
            current = previous
                .boxes
                .get(id)
                .ok_or_else(|| LayoutError::Invalid("missing previous parent".into()))?
                .parent
                .as_deref();
        }
    }
    Ok(())
}

fn node_compatible(node: &Node, old: &LayoutBox) -> bool {
    let size = old.bounds.size();
    node.parent == old.parent
        && node.solid == old.solid
        && (!node.solid
            || size.min(node.min_size) == node.min_size && size.max(node.max_size) == node.max_size)
}

fn invalid_band_members(
    nodes: &BTreeMap<&str, &Node>,
    boxes: &BTreeMap<String, LayoutBox>,
    gap: i64,
) -> BTreeSet<String> {
    let mut invalid = BTreeSet::new();
    for a in nodes.values().filter(|n| n.parent.is_some()) {
        for b in nodes
            .values()
            .filter(|b| b.parent == a.parent && b.band > a.band)
        {
            if let (Some(left), Some(right)) = (boxes.get(&a.id), boxes.get(&b.id))
                && left.bounds.max.y + gap > right.bounds.min.y
            {
                invalid.insert(a.id.clone());
                invalid.insert(b.id.clone());
            }
        }
    }
    invalid
}

fn compatible_geometry(
    nodes: &BTreeMap<&str, &Node>,
    previous: &LayoutSolution,
    pins: &BTreeMap<String, Point>,
    gap: i64,
) -> bool {
    nodes.len() == previous.boxes.len()
        && nodes.values().all(|node| {
            previous
                .boxes
                .get(&node.id)
                .is_some_and(|old| node_compatible(node, old))
        })
        && pins
            .iter()
            .all(|(id, point)| previous.boxes[id].bounds.center() == *point)
        && invalid_band_members(nodes, &previous.boxes, gap).is_empty()
}

fn affected_regions(
    nodes: &BTreeMap<&str, &Node>,
    projection: &Projection,
    previous: &LayoutSolution,
    pins: &BTreeMap<String, Point>,
    gap: i64,
) -> BTreeSet<String> {
    let mut regions = BTreeSet::new();
    // A changed child invalidates only its immediate assembly. Removed parents
    // walk upward to the nearest surviving assembly before region selection.
    let mark = |id: &str, regions: &mut BTreeSet<String>| {
        let mut current = Some(id);
        while let Some(id) = current {
            if let Some(node) = nodes.get(id) {
                regions.insert(node.parent.clone().unwrap_or_else(|| node.id.clone()));
                return;
            }
            current = previous.boxes.get(id).and_then(|b| b.parent.as_deref());
        }
        regions.extend(
            nodes
                .values()
                .filter(|n| n.parent.is_none())
                .map(|n| n.id.clone()),
        );
    };
    for node in nodes.values() {
        if previous
            .boxes
            .get(&node.id)
            .is_none_or(|old| !node_compatible(node, old))
        {
            if previous.boxes.get(&node.id).is_some_and(|old| {
                old.parent == node.parent
                    && (matches!(node.kind, NodeKind::Scope | NodeKind::Group)
                        || old.solid != node.solid)
            }) {
                // Expanding an existing assembly changes its interior. Its
                // siblings remain fixed unless the user requests reorganization.
                regions.insert(node.id.clone());
            } else {
                mark(&node.id, &mut regions);
            }
            if let Some(old) = previous.boxes.get(&node.id)
                && old.parent != node.parent
                && let Some(parent) = &old.parent
                && nodes.contains_key(parent.as_str())
            {
                regions.insert(parent.clone());
            }
        }
    }
    for old in previous
        .boxes
        .values()
        .filter(|old| !nodes.contains_key(old.id.as_str()))
    {
        // A reference becoming an expanded descendant is a representation
        // change inside that descendant's region, not removal from the root.
        let remapped: Vec<_> = old
            .semantic_ids
            .iter()
            .filter_map(|id| projection.semantic_to_display.get(id))
            .filter(|id| nodes.contains_key(id.as_str()))
            .collect();
        if remapped.len() == old.semantic_ids.len() && !remapped.is_empty() {
            for id in remapped {
                mark(id, &mut regions);
            }
            continue;
        }
        // The surviving parent is itself the changed region, not its parent.
        let mut parent = old.parent.as_deref();
        while let Some(id) = parent {
            if nodes.contains_key(id) {
                regions.insert(id.to_owned());
                break;
            }
            parent = previous.boxes.get(id).and_then(|b| b.parent.as_deref());
        }
        if parent.is_none() {
            mark(&old.id, &mut regions);
        }
    }
    for id in invalid_band_members(nodes, &previous.boxes, gap) {
        mark(&id, &mut regions);
    }
    for (id, point) in pins {
        if previous
            .boxes
            .get(id)
            .is_none_or(|old| old.bounds.center() != *point)
        {
            mark(id, &mut regions);
        }
    }
    let old_routes: BTreeMap<_, _> = previous
        .routes
        .iter()
        .map(|r| (r.edge_id.as_str(), r))
        .collect();
    for edge in &projection.edges {
        let old_route = old_routes
            .get(edge.id.as_str())
            .filter(|route| route.semantic_ids == edge.semantic_ids);
        let source_unchanged = old_route.is_some_and(|route| {
            route.points.first().is_some_and(|point| {
                previous
                    .boxes
                    .get(&edge.source)
                    .is_some_and(|object| on_box_surface(*point, object.bounds))
            })
        });
        let target_unchanged = old_route.is_some_and(|route| {
            route.points.last().is_some_and(|point| {
                previous
                    .boxes
                    .get(&edge.target)
                    .is_some_and(|object| on_box_surface(*point, object.bounds))
            })
        });
        if !source_unchanged {
            mark(&edge.source, &mut regions);
        }
        if !target_unchanged {
            mark(&edge.target, &mut regions);
        }
    }
    let edge_ids: BTreeSet<_> = projection.edges.iter().map(|e| e.id.as_str()).collect();
    for route in previous
        .routes
        .iter()
        .filter(|r| !edge_ids.contains(r.edge_id.as_str()))
    {
        for point in route.points.first().into_iter().chain(route.points.last()) {
            for object in previous
                .boxes
                .values()
                .filter(|b| b.solid && on_box_surface(*point, b.bounds))
            {
                mark(&object.id, &mut regions);
            }
        }
    }
    // A routing-only failure can be caused by a clearance profile change. Keep
    // empty, unrelated regions frozen while repairing incident assemblies.
    if regions.is_empty() {
        for edge in &projection.edges {
            mark(&edge.source, &mut regions);
            mark(&edge.target, &mut regions);
        }
    }
    let all = regions.clone();
    regions.retain(|id| {
        !all.iter()
            .any(|other| id != other && node_descendant(id, other, nodes))
    });
    regions
}

fn on_box_surface(p: Point, b: Bounds) -> bool {
    b.contains_point(p)
        && (p.x == b.min.x
            || p.x == b.max.x
            || p.y == b.min.y
            || p.y == b.max.y
            || p.z == b.min.z
            || p.z == b.max.z)
}

fn node_descendant(id: &str, ancestor: &str, nodes: &BTreeMap<&str, &Node>) -> bool {
    let mut next = Some(id);
    while let Some(id) = next {
        if id == ancestor {
            return true;
        }
        next = nodes.get(id).and_then(|n| n.parent.as_deref());
    }
    false
}

fn node_depth(id: &str, nodes: &BTreeMap<&str, &Node>) -> usize {
    let mut depth = 0;
    let mut next = nodes[id].parent.as_deref();
    while let Some(id) = next {
        depth += 1;
        next = nodes[id].parent.as_deref();
    }
    depth
}

fn restore_unaffected(
    nodes: &BTreeMap<&str, &Node>,
    boxes: &mut BTreeMap<String, LayoutBox>,
    previous: &LayoutSolution,
    affected: &BTreeSet<String>,
    iteration: usize,
) {
    for region in affected {
        if let Some(old) = previous.boxes.get(region) {
            let node = nodes[region.as_str()];
            let current = boxes[region].bounds;
            let sibling_bands: Vec<_> = nodes
                .values()
                .filter(|other| other.parent == node.parent && other.id != node.id)
                .map(|other| other.band)
                .collect();
            let has_below = sibling_bands.iter().any(|band| *band < node.band);
            let has_above = sibling_bands.iter().any(|band| *band > node.band);
            // Outer layers grow into free exterior space. The interior uses its
            // center and still fails validation if adjacent layers leave no room.
            let y = match (has_below, has_above) {
                (false, true) => old.bounds.max.y - current.max.y,
                (true, false) => old.bounds.min.y - current.min.y,
                _ => old.bounds.center().y - current.center().y,
            };
            // Eight deterministic alternatives vary only the affected region's
            // envelope; unrelated objects are restored exactly below.
            const ANCHORS: [(i8, i8); 8] = [
                (0, 0),
                (-1, -1),
                (1, 1),
                (-1, 1),
                (1, -1),
                (0, -1),
                (0, 1),
                (-1, 0),
            ];
            let (x_anchor, z_anchor) = ANCHORS[iteration % ANCHORS.len()];
            let axis = |anchor, old_min, old_max, min, max| match anchor {
                -1 => old_min - min,
                1 => old_max - max,
                _ => old_min + (old_max - old_min) / 2 - min - (max - min) / 2,
            };
            let delta = Point::new(
                axis(
                    x_anchor,
                    old.bounds.min.x,
                    old.bounds.max.x,
                    current.min.x,
                    current.max.x,
                ),
                y,
                axis(
                    z_anchor,
                    old.bounds.min.z,
                    old.bounds.max.z,
                    current.min.z,
                    current.max.z,
                ),
            );
            translate_tree(region, delta, boxes);
        }
    }
    for (id, object) in boxes {
        if !affected
            .iter()
            .any(|region| node_descendant(id, region, nodes))
            && let Some(old) = previous.boxes.get(id)
        {
            object.bounds = old.bounds;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pack_tree(
    id: &str,
    nodes: &BTreeMap<&str, &Node>,
    projection: &Projection,
    algorithm: Algorithm,
    iteration: usize,
    gap: i64,
    size_overrides: &BTreeMap<String, Point>,
    layered: Option<&LayeredOrdering>,
    boxes: &mut BTreeMap<String, LayoutBox>,
) -> Result<(), LayoutError> {
    let node = nodes[id];
    let mut children: Vec<_> = nodes
        .values()
        .filter(|n| n.parent.as_deref() == Some(id))
        .map(|n| n.id.clone())
        .collect();
    children.sort();
    if children.is_empty() {
        let mut size = size_overrides
            .get(id)
            .copied()
            .unwrap_or(node.preferred_size);
        if algorithm == Algorithm::Floorplan
            && iteration % 5 == 4
            && node.kind != NodeKind::Concern
            && !size_overrides.contains_key(id)
        {
            let swapped = Point::new(size.z, size.y, size.x);
            if swapped.min(node.min_size) == node.min_size
                && swapped.max(node.max_size) == node.max_size
            {
                size = swapped;
            }
        }
        boxes.insert(
            id.into(),
            LayoutBox {
                id: id.into(),
                semantic_ids: node.semantic_ids.clone(),
                parent: node.parent.clone(),
                bounds: Bounds {
                    min: Point::ZERO,
                    max: size,
                },
                solid: node.solid,
            },
        );
        return Ok(());
    }
    for child in &children {
        pack_tree(
            child,
            nodes,
            projection,
            algorithm,
            iteration,
            gap,
            size_overrides,
            layered,
            boxes,
        )?;
    }
    // Solid parents are components with explicit attached interfaces.
    if node.solid {
        let h = children
            .iter()
            .map(|c| boxes[c].bounds.size().y + gap / 2)
            .sum::<i64>()
            .max(node.preferred_size.y);
        let width = children
            .iter()
            .map(|c| boxes[c].bounds.size().x)
            .max()
            .unwrap_or(0)
            .max(node.preferred_size.x);
        let size = Point::new(width, h, node.preferred_size.z);
        let mut y = 0;
        for child in &children {
            let b = boxes[child].bounds;
            translate_tree(
                child,
                Point::new((width - b.size().x) / 2, y, size.z) - b.min,
                boxes,
            );
            y += b.size().y + gap / 2;
        }
        boxes.insert(
            id.into(),
            LayoutBox {
                id: id.into(),
                semantic_ids: node.semantic_ids.clone(),
                parent: node.parent.clone(),
                bounds: Bounds {
                    min: Point::ZERO,
                    max: size,
                },
                solid: true,
            },
        );
        return Ok(());
    }
    let mut bands: BTreeMap<i32, Vec<String>> = BTreeMap::new();
    for child in &children {
        bands
            .entry(nodes[child.as_str()].band)
            .or_default()
            .push(child.clone());
    }
    if let Some(ordering) = layered {
        for members in bands.values_mut() {
            members.sort_by_key(|id| ordering.ranks[id]);
        }
    }
    let layered_width = if algorithm == Algorithm::Layered {
        bands
            .values()
            .map(|members| {
                members
                    .iter()
                    .map(|id| {
                        let size = subtree_bounds(id, boxes).size();
                        (size.x + gap) as f64 * (size.z + gap) as f64
                    })
                    .sum::<f64>()
                    .sqrt()
                    .ceil() as i64
            })
            .max()
            .unwrap_or(1)
    } else {
        0
    };
    let mut y = 0;
    for members in bands.values() {
        let rects: Vec<_> = members
            .iter()
            .map(|c| {
                let b = subtree_bounds(c, boxes);
                (c.clone(), b.size())
            })
            .collect();
        let positions = match algorithm {
            Algorithm::Floorplan if rects.len() >= 8 && iteration % 8 >= 6 => {
                stacked_sequence_seed(&rects, nodes, projection, iteration, gap)
            }
            Algorithm::Floorplan => sequence_seed(&rects, iteration, gap),
            Algorithm::Layered => layered_depth_seed(&rects, layered_width, iteration, gap),
            Algorithm::Treemap => {
                grid_seed(&rects, (rects.len() as f64).sqrt().ceil() as usize, gap)
            }
            Algorithm::Constrained => constrained_seed(&rects, projection, iteration, gap),
        };
        let max_y = rects
            .iter()
            .map(|(id, size)| positions[id].y + size.y)
            .max()
            .unwrap_or(0);
        for (child, p) in positions {
            let old = subtree_bounds(&child, boxes);
            translate_tree(&child, Point::new(p.x, y + p.y, p.z) - old.min, boxes);
        }
        y += max_y + gap;
    }
    if matches!(
        algorithm,
        Algorithm::Floorplan | Algorithm::Constrained | Algorithm::Layered
    ) {
        align_bands(&bands, projection, boxes, gap);
        if matches!(algorithm, Algorithm::Floorplan | Algorithm::Layered) {
            reserve_passages(&bands, projection, boxes, gap);
        }
    }
    let bounds = children
        .iter()
        .map(|c| subtree_bounds(c, boxes))
        .reduce(|a, b| Bounds {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })
        .unwrap()
        .inflated(gap / 2);
    boxes.insert(
        id.into(),
        LayoutBox {
            id: id.into(),
            semantic_ids: node.semantic_ids.clone(),
            parent: node.parent.clone(),
            bounds,
            solid: false,
        },
    );
    Ok(())
}

struct LayeredOrdering {
    ranks: BTreeMap<String, usize>,
    sweeps: usize,
    comparisons: usize,
    comparison_limit: usize,
}

type LayerBands = BTreeMap<i32, Vec<String>>;
type LayerNeighbors = BTreeMap<String, BTreeSet<String>>;

/// The projection has already assigned responsibility/SCC bands. Reduce order
/// crossings within those fixed bands; this is a bounded layered initializer,
/// not an implementation of ELK or a claim of crossing optimality.
fn layered_ordering(
    nodes: &BTreeMap<&str, &Node>,
    projection: &Projection,
    budget: usize,
    cancelled: &AtomicBool,
) -> Result<LayeredOrdering, LayoutError> {
    let sweep_limit = budget.min(8);
    let mut result = LayeredOrdering {
        ranks: BTreeMap::new(),
        sweeps: 0,
        comparisons: 0,
        comparison_limit: nodes.len() * sweep_limit * 64,
    };
    let mut assemblies = BTreeMap::<String, LayerBands>::new();
    for node in nodes.values() {
        check_cancel(cancelled)?;
        if let Some(parent) = &node.parent {
            assemblies
                .entry(parent.clone())
                .or_default()
                .entry(node.band)
                .or_default()
                .push(node.id.clone());
        }
    }
    let mut neighbors = LayerNeighbors::new();
    for edge in &projection.edges {
        // Lift both endpoints to siblings at their common containing assembly.
        // Internal claims then influence the correct parent envelope's order.
        let mut source_path = BTreeMap::new();
        let mut source = edge.source.as_str();
        while let Some(parent) = nodes[source].parent.as_deref() {
            check_cancel(cancelled)?;
            source_path.insert(parent, source);
            source = parent;
        }
        let mut target = edge.target.as_str();
        while let Some(parent) = nodes[target].parent.as_deref() {
            check_cancel(cancelled)?;
            if let Some(source) = source_path.get(parent) {
                if *source != target {
                    neighbors
                        .entry((*source).into())
                        .or_default()
                        .insert(target.into());
                    neighbors
                        .entry(target.into())
                        .or_default()
                        .insert((*source).into());
                }
                break;
            }
            target = parent;
        }
    }
    for mut bands in assemblies.into_values() {
        check_cancel(cancelled)?;
        let mut best = bands.clone();
        let mut best_cost = layered_order_cost(&bands, &neighbors, cancelled)?;
        let mut ranks = layer_ranks(&bands);
        for sweep in 0..sweep_limit {
            result.sweeps += 1;
            let mut band_ids: Vec<_> = bands.keys().copied().collect();
            if sweep % 2 == 1 {
                band_ids.reverse();
            }
            for band in band_ids {
                check_cancel(cancelled)?;
                let mut scores = BTreeMap::new();
                for id in &bands[&band] {
                    let mut total = 0_u64;
                    let mut count = 0;
                    if let Some(related) = neighbors.get(id) {
                        for neighbor in related {
                            check_cancel(cancelled)?;
                            let other_band = nodes[neighbor.as_str()].band;
                            if (sweep % 2 == 0 && other_band < band)
                                || (sweep % 2 == 1 && other_band > band)
                            {
                                total += ((2 * ranks[neighbor] + 1) * 65_536
                                    / (2 * bands[&other_band].len()))
                                    as u64;
                                count += 1;
                            }
                        }
                        // Peer-only regions still benefit from short local spans.
                        if count == 0 {
                            for neighbor in related {
                                check_cancel(cancelled)?;
                                if nodes[neighbor.as_str()].band == band {
                                    total += ((2 * ranks[neighbor] + 1) * 65_536
                                        / (2 * bands[&band].len()))
                                        as u64;
                                    count += 1;
                                }
                            }
                        }
                    }
                    let score = total.checked_div(count).unwrap_or_else(|| {
                        ((2 * ranks[id] + 1) * 65_536 / (2 * bands[&band].len())) as u64
                    });
                    scores.insert(id.clone(), score);
                }
                bands
                    .get_mut(&band)
                    .unwrap()
                    .sort_by_key(|id| (scores[id], ranks[id]));
                ranks = layer_ranks(&bands);
                // Strict local improvements only, with an explicit neighbor-work
                // ceiling even for dense hubs. Exhaustion keeps a legal ordering.
                for i in 1..bands[&band].len() {
                    let members = &bands[&band];
                    let a = &members[i - 1];
                    let b = &members[i];
                    if layered_swap_improves(
                        a,
                        b,
                        &neighbors,
                        nodes,
                        &ranks,
                        &mut result,
                        cancelled,
                    )? {
                        bands.get_mut(&band).unwrap().swap(i - 1, i);
                        ranks = layer_ranks(&bands);
                    }
                }
            }
            let cost = layered_order_cost(&bands, &neighbors, cancelled)?;
            if cost < best_cost {
                best_cost = cost;
                best = bands.clone();
            }
        }
        result.ranks.extend(layer_ranks(&best));
    }
    check_cancel(cancelled)?;
    Ok(result)
}

fn layer_ranks(bands: &LayerBands) -> BTreeMap<String, usize> {
    bands
        .values()
        .flat_map(|ids| ids.iter().cloned().enumerate().map(|(i, id)| (id, i)))
        .collect()
}

/// Count ordering inversions for each pair of bands in O(E log N). Equal source
/// or target ranks share an endpoint and do not count as a crossing. Same-band
/// spans are a secondary objective; no architectural direction is rewritten.
fn layered_order_cost(
    bands: &LayerBands,
    neighbors: &LayerNeighbors,
    cancelled: &AtomicBool,
) -> Result<(usize, usize), LayoutError> {
    let ranks = layer_ranks(bands);
    let band_of: BTreeMap<_, _> = bands
        .iter()
        .flat_map(|(band, ids)| ids.iter().map(move |id| (id, *band)))
        .collect();
    let mut pairs = BTreeMap::<(i32, i32), Vec<(usize, usize)>>::new();
    let mut spans = 0;
    for (a, rank) in &ranks {
        check_cancel(cancelled)?;
        if let Some(related) = neighbors.get(a) {
            for b in related.iter().filter(|b| *b > a) {
                check_cancel(cancelled)?;
                if band_of[a] == band_of[b] {
                    spans += rank.abs_diff(ranks[b]);
                } else if band_of[a] < band_of[b] {
                    pairs
                        .entry((band_of[a], band_of[b]))
                        .or_default()
                        .push((*rank, ranks[b]));
                } else {
                    pairs
                        .entry((band_of[b], band_of[a]))
                        .or_default()
                        .push((ranks[b], *rank));
                }
            }
        }
    }
    let mut crossings = 0;
    for ((_, target_band), mut edges) in pairs {
        check_cancel(cancelled)?;
        edges.sort_unstable();
        let mut tree = vec![0_usize; bands[&target_band].len() + 1];
        let mut inserted = 0;
        let mut start = 0;
        while start < edges.len() {
            check_cancel(cancelled)?;
            let end = start + edges[start..].partition_point(|edge| edge.0 == edges[start].0);
            for (_, rank) in &edges[start..end] {
                let mut i = rank + 1;
                let mut at_or_before = 0;
                while i > 0 {
                    at_or_before += tree[i];
                    i &= i - 1;
                }
                crossings += inserted - at_or_before;
            }
            for (_, rank) in &edges[start..end] {
                let mut i = rank + 1;
                while i < tree.len() {
                    tree[i] += 1;
                    i += i & i.wrapping_neg();
                }
                inserted += 1;
            }
            start = end;
        }
    }
    Ok((crossings, spans))
}

#[allow(clippy::too_many_arguments)]
fn layered_swap_improves(
    a: &str,
    b: &str,
    neighbors: &LayerNeighbors,
    nodes: &BTreeMap<&str, &Node>,
    ranks: &BTreeMap<String, usize>,
    work: &mut LayeredOrdering,
    cancelled: &AtomicBool,
) -> Result<bool, LayoutError> {
    check_cancel(cancelled)?;
    if work.comparisons >= work.comparison_limit {
        return Ok(false);
    }
    let empty = BTreeSet::new();
    let left = neighbors.get(a).unwrap_or(&empty);
    let right = neighbors.get(b).unwrap_or(&empty);
    let band = nodes[a].band;
    let (mut before, mut after) = (0, 0);
    for x in left {
        for y in right {
            check_cancel(cancelled)?;
            if work.comparisons == work.comparison_limit {
                return Ok(false);
            }
            work.comparisons += 1;
            if nodes[x.as_str()].band != band && nodes[y.as_str()].band == nodes[x.as_str()].band {
                before += usize::from(ranks[x] > ranks[y]);
                after += usize::from(ranks[x] < ranks[y]);
            }
        }
    }
    let (mut before_span, mut after_span) = (0, 0);
    for (id, other, related) in [(a, b, left), (b, a, right)] {
        for neighbor in related
            .iter()
            .filter(|n| n.as_str() != other && nodes[n.as_str()].band == band)
        {
            check_cancel(cancelled)?;
            if work.comparisons == work.comparison_limit {
                return Ok(false);
            }
            work.comparisons += 1;
            before_span += ranks[id].abs_diff(ranks[neighbor]);
            after_span += ranks[other].abs_diff(ranks[neighbor]);
        }
    }
    Ok((after, after_span) < (before, before_span))
}

/// Variable rectangles wrap into depth rows without overlap. All bands use a
/// common target width; bounded aspect/orientation alternatives remain genuine
/// placements, never changes to the supplied dimensions or semantic layers.
fn layered_depth_seed(
    rects: &[(String, Point)],
    width: i64,
    iteration: usize,
    gap: i64,
) -> BTreeMap<String, Point> {
    const PERCENT: [i64; 7] = [100, 80, 125, 60, 160, 200, 0];
    let transpose = iteration / PERCENT.len() % 2 == 1;
    let factor = PERCENT[iteration % PERCENT.len()];
    let width = if factor == 0 {
        i64::MAX
    } else {
        (width * factor / 100).max(1)
    };
    let (mut x, mut z, mut depth) = (0, 0, 0);
    let mut result = BTreeMap::new();
    for (id, size) in rects {
        let (w, d) = if transpose {
            (size.z, size.x)
        } else {
            (size.x, size.z)
        };
        if x > 0 && x + w > width {
            x = 0;
            z += depth + gap;
            depth = 0;
        }
        let position = if transpose {
            Point::new(z, 0, x)
        } else {
            Point::new(x, 0, z)
        };
        result.insert(id.clone(), position);
        x += w + gap;
        depth = depth.max(d);
    }
    result
}

fn grid_seed(rects: &[(String, Point)], columns: usize, gap: i64) -> BTreeMap<String, Point> {
    let mut result = BTreeMap::new();
    let mut z = 0;
    for row in rects.chunks(columns.max(1)) {
        let mut x = 0;
        let depth = row.iter().map(|r| r.1.z).max().unwrap_or(0);
        for (id, size) in row {
            result.insert(id.clone(), Point::new(x, 0, z));
            x += size.x + gap;
        }
        z += depth + gap;
    }
    result
}

/// Decode a sequence pair into longest paths in the two separation DAGs.
/// Every pair is separated on x or z; there is no overlap-removal approximation.
pub fn decode_sequence_pair(
    rects: &[(String, Point)],
    positive: &[usize],
    negative: &[usize],
    gap: i64,
) -> BTreeMap<String, Point> {
    let mut rank = vec![0; rects.len()];
    for (i, j) in negative.iter().enumerate() {
        rank[*j] = i;
    }
    let mut positions = vec![Point::ZERO; rects.len()];
    for (i, a) in positive.iter().enumerate() {
        for b in &positive[..i] {
            if rank[*b] < rank[*a] {
                positions[*a].x = positions[*a].x.max(positions[*b].x + rects[*b].1.x + gap);
            } else {
                positions[*a].z = positions[*a].z.max(positions[*b].z + rects[*b].1.z + gap);
            }
        }
    }
    rects
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.clone(), positions[i]))
        .collect()
}

fn sequence_seed(rects: &[(String, Point)], iteration: usize, gap: i64) -> BTreeMap<String, Point> {
    let n = rects.len();
    let columns = ((n as f64).sqrt().ceil() as usize + (iteration / 8) % 3)
        .min(n)
        .max(1);
    let mut positive: Vec<_> = (0..n).collect();
    let mut negative: Vec<_> = (0..n.div_ceil(columns))
        .rev()
        .flat_map(|r| r * columns..((r + 1) * columns).min(n))
        .collect();
    if n > 1 && iteration > 0 {
        let mut rng = (iteration as u64)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        for _ in 0..1 + iteration % 4 {
            rng = rng
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            let a = rng as usize % n;
            rng = rng.rotate_left(17);
            let b = rng as usize % n;
            if iteration.is_multiple_of(2) {
                positive.swap(a, b);
            } else {
                negative.swap(a, b);
            }
        }
    }
    decode_sequence_pair(rects, &positive, &negative, gap)
}

/// Same-responsibility peers may occupy several physical shelves. These are
/// presentation positions, not invented architectural ranks. Each shelf keeps
/// sequence-pair separation; disjoint y ranges separate shelves. The caller
/// starts the next responsibility band above the entire resulting envelope.
fn stacked_sequence_seed(
    rects: &[(String, Point)],
    nodes: &BTreeMap<&str, &Node>,
    projection: &Projection,
    iteration: usize,
    gap: i64,
) -> BTreeMap<String, Point> {
    let padded_volume: f64 = rects
        .iter()
        .map(|(_, s)| (s.x + gap) as f64 * (s.y + gap) as f64 * (s.z + gap) as f64)
        .sum();
    let height = rects.iter().map(|(_, s)| s.y).max().unwrap_or(1) + gap;
    let target_shelves = (padded_volume.cbrt() / height as f64).round() as usize;
    let shelves = if iteration % 8 == 6 {
        target_shelves
    } else {
        target_shelves.saturating_add(1)
    }
    .clamp(2, rects.len());
    let per_shelf = rects.len().div_ceil(shelves);
    // Put peers connected to higher responsibility bands near the top face,
    // and peers connected below near the bottom. Sorting by ID alone could bury
    // an auth-covered middleware beneath unrelated same-band components.
    // Normalize each neighbor's pull so one ubiquitous hub cannot dominate.
    let mut degree = BTreeMap::<&str, i64>::new();
    for edge in &projection.edges {
        *degree.entry(&edge.source).or_default() += 1;
        *degree.entry(&edge.target).or_default() += 1;
    }
    let mut ordered = rects.to_vec();
    ordered.sort_by_cached_key(|(id, _)| {
        let rank = nodes[id.as_str()].band;
        let parent = &nodes[id.as_str()].parent;
        let affinity: i64 = projection
            .edges
            .iter()
            .filter_map(|edge| {
                let neighbor = if edge.source == *id {
                    &edge.target
                } else if edge.target == *id {
                    &edge.source
                } else {
                    return None;
                };
                let other = nodes.get(neighbor.as_str())?;
                (other.parent == *parent).then_some(
                    i64::from((other.band - rank).signum())
                        * (1024 / degree[neighbor.as_str()]).max(1),
                )
            })
            .sum();
        (affinity, id.clone())
    });
    let mut result = BTreeMap::new();
    let mut y = 0;
    for shelf in ordered.chunks(per_shelf) {
        // Earlier iterations keep a compact deterministic row ordering; later
        // iterations explore the existing local sequence-pair perturbations.
        let positions = sequence_seed(shelf, iteration / 8, gap);
        for (id, p) in positions {
            result.insert(id, Point::new(p.x, y, p.z));
        }
        y += shelf.iter().map(|(_, s)| s.y).max().unwrap_or(0) + gap;
    }
    result
}

fn constrained_seed(
    rects: &[(String, Point)],
    projection: &Projection,
    iteration: usize,
    gap: i64,
) -> BTreeMap<String, Point> {
    let mut p = grid_seed(rects, (rects.len() as f64).sqrt().ceil() as usize, gap);
    let sizes: BTreeMap<_, _> = rects.iter().cloned().collect();
    for _ in 0..4 + iteration % 4 {
        for (id, size) in rects {
            let neighbors: Vec<_> = projection
                .edges
                .iter()
                .filter_map(|e| {
                    if e.source == *id {
                        p.get(&e.target)
                    } else if e.target == *id {
                        p.get(&e.source)
                    } else {
                        None
                    }
                })
                .copied()
                .collect();
            if neighbors.is_empty() {
                continue;
            }
            let target = Point::new(
                neighbors.iter().map(|v| v.x).sum::<i64>() / neighbors.len() as i64,
                0,
                neighbors.iter().map(|v| v.z).sum::<i64>() / neighbors.len() as i64,
            );
            let current = p[id];
            let next = Point::new((current.x + target.x) / 2, 0, (current.z + target.z) / 2);
            let candidate = Bounds {
                min: next,
                max: next + *size,
            }
            .inflated(gap / 2);
            if p.iter().all(|(other, pos)| {
                other == id
                    || !candidate.overlaps(
                        Bounds {
                            min: *pos,
                            max: *pos + sizes[other],
                        }
                        .inflated(gap / 2),
                    )
            }) {
                p.insert(id.clone(), next);
            }
        }
    }
    p
}

fn align_bands(
    bands: &BTreeMap<i32, Vec<String>>,
    projection: &Projection,
    boxes: &mut BTreeMap<String, LayoutBox>,
    gap: i64,
) {
    // The dense middle band anchors the assembly. Adjacent bands follow their
    // connected neighbors, with deterministic collision rejection.
    let anchor = bands
        .iter()
        .max_by_key(|(band, m)| (m.len(), -**band))
        .map(|(b, _)| *b)
        .unwrap_or(0);
    let mut order: Vec<_> = bands.keys().copied().filter(|b| *b != anchor).collect();
    order.sort_by_key(|b| ((b - anchor).abs(), *b));
    for band in order {
        for id in &bands[&band] {
            let related: Vec<_> = projection
                .edges
                .iter()
                .filter_map(|e| {
                    let neighbor = if e.source == *id {
                        &e.target
                    } else if e.target == *id {
                        &e.source
                    } else {
                        return None;
                    };
                    if bands[&band].contains(neighbor) {
                        None
                    } else {
                        boxes.get(neighbor).map(|b| b.bounds.center())
                    }
                })
                .collect();
            if related.is_empty() {
                continue;
            }
            let center = boxes[id].bounds.center();
            let target = Point::new(
                related.iter().map(|p| p.x).sum::<i64>() / related.len() as i64,
                center.y,
                related.iter().map(|p| p.z).sum::<i64>() / related.len() as i64,
            );
            let shift = target - center;
            let candidate = subtree_bounds(id, boxes)
                .translated(shift)
                .inflated(gap / 2);
            if bands[&band].iter().all(|other| {
                other == id || !candidate.overlaps(subtree_bounds(other, boxes).inflated(gap / 2))
            }) {
                translate_tree(id, shift, boxes);
            }
        }
    }
}
// A single-purpose endpoint in another band or physical shelf gets a vertical
// passage. Reposition its peers, then clear every geometrically intervening
// sibling, including endpoint-band shelves, before routing.
// This is based on graph structure rather than IDs from the sketch fixture.
fn reserve_passages(
    bands: &BTreeMap<i32, Vec<String>>,
    projection: &Projection,
    boxes: &mut BTreeMap<String, LayoutBox>,
    gap: i64,
) {
    let ranks: BTreeMap<_, _> = bands
        .iter()
        .flat_map(|(rank, ids)| ids.iter().map(move |id| (id.as_str(), *rank)))
        .collect();
    let mut reserved = Vec::<Bounds>::new();
    let mut edges: Vec<_> = projection.edges.iter().collect();
    edges.sort_by_key(|e| &e.id);
    for edge in edges {
        let (Some(a), Some(b)) = (
            ranks.get(edge.source.as_str()),
            ranks.get(edge.target.as_str()),
        ) else {
            continue;
        };
        let source = subtree_bounds(&edge.source, boxes);
        let target = subtree_bounds(&edge.target, boxes);
        if a == b && !(source.max.y + gap <= target.min.y || target.max.y + gap <= source.min.y) {
            continue;
        }
        let degree = |id: &str| {
            projection
                .edges
                .iter()
                .filter(|e| e.source == id || e.target == id)
                .count()
        };
        let (moving, fixed, rank) = if degree(&edge.source) == 1 {
            (&edge.source, &edge.target, *a)
        } else if degree(&edge.target) == 1 {
            (&edge.target, &edge.source, *b)
        } else {
            continue;
        };
        let old = boxes[moving].bounds.center();
        let other = boxes[fixed].bounds.center();
        let next = Point::new(other.x, old.y, other.z);
        let shift = next - old;
        let candidate = subtree_bounds(moving, boxes)
            .translated(shift)
            .inflated(gap / 2);
        let collisions: Vec<_> = bands[&rank]
            .iter()
            .filter(|id| {
                *id != moving && candidate.overlaps(subtree_bounds(id, boxes).inflated(gap / 2))
            })
            .cloned()
            .collect();
        translate_tree(moving, shift, boxes);
        for id in collisions {
            let current = boxes[&id].bounds.center();
            let mut target = Point::new(old.x, current.y, old.z);
            for _ in 0..bands[&rank].len() + 1 {
                let trial = subtree_bounds(&id, boxes)
                    .translated(target - current)
                    .inflated(gap / 2);
                if bands[&rank].iter().all(|peer| {
                    *peer == id || !trial.overlaps(subtree_bounds(peer, boxes).inflated(gap / 2))
                }) {
                    break;
                }
                target.x += subtree_bounds(&id, boxes).size().x + gap;
            }
            translate_tree(&id, target - current, boxes);
        }
        let start = boxes[moving].bounds;
        let end = boxes[fixed].bounds;
        let shaft = Bounds {
            min: Point::new(
                other.x - gap / 2,
                start.min.y.min(end.min.y),
                other.z - gap / 2,
            ),
            max: Point::new(
                other.x + gap / 2,
                start.max.y.max(end.max.y),
                other.z + gap / 2,
            ),
        };
        reserved.push(shaft);
        for (_, peers) in bands.range(*a.min(b)..=*a.max(b)) {
            for id in peers {
                if id == moving || id == fixed || !subtree_bounds(id, boxes).overlaps(shaft) {
                    continue;
                }
                let original = subtree_bounds(id, boxes);
                let choices = [
                    Point::new(shaft.max.x + gap / 2 - original.min.x, 0, 0),
                    Point::new(shaft.min.x - gap / 2 - original.max.x, 0, 0),
                    Point::new(0, 0, shaft.max.z + gap / 2 - original.min.z),
                    Point::new(0, 0, shaft.min.z - gap / 2 - original.max.z),
                ];
                let best = choices
                    .into_iter()
                    .filter(|delta| {
                        let trial = original.translated(*delta).inflated(gap / 2);
                        peers.iter().all(|peer| {
                            peer == id
                                || !trial.overlaps(subtree_bounds(peer, boxes).inflated(gap / 2))
                        }) && reserved.iter().all(|shaft| !trial.overlaps(*shaft))
                    })
                    .min_by_key(|delta| delta.manhattan(Point::ZERO));
                if let Some(delta) = best {
                    translate_tree(id, delta, boxes);
                }
            }
        }
    }
}
fn is_descendant(id: &str, parent: &str, boxes: &BTreeMap<String, LayoutBox>) -> bool {
    let mut current = Some(id);
    while let Some(id) = current {
        if id == parent {
            return true;
        }
        current = boxes.get(id).and_then(|b| b.parent.as_deref());
    }
    false
}
fn subtree_bounds(id: &str, boxes: &BTreeMap<String, LayoutBox>) -> Bounds {
    boxes
        .values()
        .filter(|b| is_descendant(&b.id, id, boxes))
        .map(|b| b.bounds)
        .reduce(|a, b| Bounds {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })
        .unwrap()
}
fn translate_tree(id: &str, shift: Point, boxes: &mut BTreeMap<String, LayoutBox>) {
    let ids: Vec<_> = boxes
        .keys()
        .filter(|k| is_descendant(k, id, boxes))
        .cloned()
        .collect();
    for k in ids {
        let b = boxes.get_mut(&k).unwrap();
        b.bounds = b.bounds.translated(shift);
    }
}
fn refit_frames(
    nodes: &BTreeMap<&str, &Node>,
    boxes: &mut BTreeMap<String, LayoutBox>,
    padding: i64,
    pins: &BTreeMap<String, Point>,
    preserve_envelope: bool,
) {
    let mut ids: Vec<_> = nodes
        .values()
        .filter(|n| !n.solid)
        .map(|n| n.id.clone())
        .collect();
    ids.sort_by_key(|id| {
        let mut d = 0;
        let mut p = nodes[id.as_str()].parent.as_deref();
        while let Some(id) = p {
            d += 1;
            p = nodes[id].parent.as_deref();
        }
        std::cmp::Reverse(d)
    });
    for id in ids {
        let children: Vec<_> = boxes
            .values()
            .filter(|b| b.parent.as_deref() == Some(&id))
            .map(|b| b.id.clone())
            .collect();
        if let Some(bounds) = children
            .iter()
            .map(|c| subtree_bounds(c, boxes))
            .reduce(|a, b| Bounds {
                min: a.min.min(b.min),
                max: a.max.max(b.max),
            })
        {
            let mut bounds = bounds.inflated(padding);
            if preserve_envelope {
                let old = boxes[&id].bounds;
                bounds = Bounds {
                    min: bounds.min.min(old.min),
                    max: bounds.max.max(old.max),
                };
            }
            if let Some(&center) = pins.get(&id) {
                let radius = Point::new(
                    (center.x - bounds.min.x)
                        .abs()
                        .max((bounds.max.x - center.x).abs()),
                    (center.y - bounds.min.y)
                        .abs()
                        .max((bounds.max.y - center.y).abs()),
                    (center.z - bounds.min.z)
                        .abs()
                        .max((bounds.max.z - center.z).abs()),
                );
                bounds = Bounds {
                    min: center - radius,
                    max: center + radius,
                };
            }
            boxes.get_mut(&id).unwrap().bounds = bounds;
        }
    }
}
fn estimate(projection: &Projection, boxes: &BTreeMap<String, LayoutBox>) -> f64 {
    let wires = projection
        .edges
        .iter()
        .map(|e| {
            boxes[&e.source]
                .bounds
                .center()
                .manhattan(boxes[&e.target].bounds.center()) as f64
        })
        .sum::<f64>();
    let b = boxes
        .values()
        .map(|b| b.bounds)
        .reduce(|a, b| Bounds {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })
        .unwrap();
    let s = b.size();
    wires
        + 0.1 * (s.x as f64 * s.y as f64 * s.z as f64).cbrt()
        + 2.0 * (s.x.max(s.y).max(s.z) - s.x.min(s.y).min(s.z)) as f64
}

#[cfg(test)]
mod layered_tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, band: i32, solid: bool) -> Node {
        Node {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            kind: if solid {
                NodeKind::Service
            } else {
                NodeKind::Scope
            },
            band,
            min_size: Point::new(10, 10, 10),
            preferred_size: Point::new(40, 40, 40),
            max_size: Point::new(80, 80, 80),
            solid,
        }
    }

    fn projection() -> Projection {
        Projection {
            key: "layered-test".into(),
            model_id: "layered-test".into(),
            revision: 1,
            root: "root".into(),
            nodes: vec![node("root", None, 0, false)],
            edges: vec![],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        }
    }

    fn edge(p: &mut Projection, id: &str, source: &str, target: &str) {
        p.edges.push(Edge {
            bundle: None,
            id: id.into(),
            source: source.into(),
            target: target.into(),
            semantic_ids: vec![id.into()],
        });
    }

    fn solve_layered(
        p: &Projection,
        pins: &BTreeMap<String, Point>,
    ) -> Result<LayoutSolution, LayoutError> {
        solve(LayoutRequest {
            projection: p,
            algorithm: Algorithm::Layered,
            pins,
            previous: None,
            quality: QualityProfile {
                feedback_candidates: 0,
                size_candidates: 0,
                ..QualityProfile::default()
            },
            work_budget: 16,
            cancelled: &AtomicBool::new(false),
        })
    }

    #[test]
    fn layered_reduces_reversed_crossings_including_nested_endpoint_claims() {
        for nested in [false, true] {
            let mut p = projection();
            for i in 0..4 {
                for (prefix, band) in [("a", 0), ("b", 1)] {
                    let id = format!("{prefix}{i}");
                    p.nodes.push(node(&id, Some("root"), band, !nested));
                    if nested {
                        p.nodes
                            .push(node(&format!("{id}:leaf"), Some(&id), 0, true));
                    }
                }
                let suffix = if nested { ":leaf" } else { "" };
                edge(
                    &mut p,
                    &format!("edge:{i}"),
                    &format!("b{i}{suffix}"),
                    &format!("a{}{suffix}", 3 - i),
                );
            }
            let nodes = p.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
            for budget in [1, 8] {
                let order = layered_ordering(&nodes, &p, budget, &AtomicBool::new(false)).unwrap();
                for i in 0..4 {
                    assert_eq!(
                        order.ranks[&format!("b{i}")],
                        order.ranks[&format!("a{}", 3 - i)]
                    );
                }
                assert!(order.comparisons <= order.comparison_limit);
                assert!(order.sweeps <= p.nodes.len() * budget.min(8));
            }
            let solved = solve_layered(&p, &BTreeMap::new()).unwrap();
            validate(&p, &solved, &BTreeMap::new(), 2).unwrap();
            assert_eq!(solved.routes.len(), p.edges.len());
            let mut reordered = p.clone();
            reordered.nodes.reverse();
            reordered.edges.reverse();
            assert_eq!(solved, solve_layered(&reordered, &BTreeMap::new()).unwrap());
        }
    }

    #[test]
    fn layered_depth_packs_variable_rectangles_on_both_horizontal_axes() {
        let mut p = projection();
        for i in 0..16 {
            let mut n = node(&format!("peer:{i:02}"), Some("root"), 0, true);
            n.preferred_size = Point::new(40 + i % 3 * 4, 30, 20 + i % 5 * 5);
            p.nodes.push(n);
        }
        let solution = solve_layered(&p, &BTreeMap::new()).unwrap();
        validate(&p, &solution, &BTreeMap::new(), 2).unwrap();
        for n in p.nodes.iter().filter(|n| n.solid) {
            assert_eq!(solution.boxes[&n.id].bounds.size(), n.preferred_size);
        }
        let x: BTreeSet<_> = solution
            .boxes
            .values()
            .filter(|b| b.solid)
            .map(|b| b.bounds.min.x)
            .collect();
        let z: BTreeSet<_> = solution
            .boxes
            .values()
            .filter(|b| b.solid)
            .map(|b| b.bounds.min.z)
            .collect();
        assert!(
            x.len() > 1 && z.len() > 1,
            "a layered band must have real depth packing"
        );
        let size = solution.boxes["root"].bounds.size();
        assert!(size.x.max(size.z) <= size.x.min(size.z) * 2);
    }

    #[test]
    fn layered_auth_alignment_pins_and_invalid_constraints_use_shared_validation() {
        let mut p = projection();
        let mut auth = node("auth", Some("root"), 2, true);
        auth.kind = NodeKind::Concern;
        p.nodes
            .extend([auth, node("middleware", Some("root"), 1, true)]);
        edge(&mut p, "coverage", "auth", "middleware");
        let solution = solve_layered(&p, &BTreeMap::new()).unwrap();
        let route = &solution.routes[0].points;
        assert_eq!(route.len(), 2);
        assert_eq!(route[0].x, route[1].x);
        assert_eq!(route[0].z, route[1].z);
        assert!(route[0].y > route[1].y);
        let pins: BTreeMap<_, _> = ["auth", "middleware"]
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    solution.boxes[id].bounds.center() + Point::new(1000, 200, -400),
                )
            })
            .collect();
        let pinned = solve_layered(&p, &pins).unwrap();
        validate(&p, &pinned, &pins, 2).unwrap();
        for (id, center) in &pins {
            assert_eq!(pinned.boxes[id].bounds.center(), *center);
        }
        let conflicting = BTreeMap::from([
            ("auth".into(), Point::ZERO),
            ("middleware".into(), Point::ZERO),
        ]);
        assert!(solve_layered(&p, &conflicting).is_err());
        let nodes = p.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert!(matches!(
            layered_ordering(&nodes, &p, 8, &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
    }

    #[test]
    fn layered_order_cost_excludes_shared_endpoints_from_crossing_count() {
        let bands = BTreeMap::from([
            (0, vec!["a".into(), "b".into()]),
            (1, vec!["c".into(), "d".into()]),
        ]);
        let mut neighbors = LayerNeighbors::new();
        for (a, b) in [("a", "c"), ("a", "d"), ("b", "c"), ("b", "d")] {
            neighbors.entry(a.into()).or_default().insert(b.into());
            neighbors.entry(b.into()).or_default().insert(a.into());
        }
        assert_eq!(
            layered_order_cost(&bands, &neighbors, &AtomicBool::new(false)).unwrap(),
            (1, 0)
        );
    }

    #[test]
    fn layered_cycles_parallel_claims_and_disconnected_peers_remain_truthful() {
        let mut p = projection();
        for id in ["a", "b", "c", "disconnected"] {
            p.nodes.push(node(id, Some("root"), 0, true));
        }
        for (id, source, target) in [
            ("ab", "a", "b"),
            ("ab-parallel", "a", "b"),
            ("bc", "b", "c"),
            ("ca", "c", "a"),
            ("ba", "b", "a"),
        ] {
            edge(&mut p, id, source, target);
        }
        let solved = solve_layered(&p, &BTreeMap::new()).unwrap();
        validate(&p, &solved, &BTreeMap::new(), 2).unwrap();
        assert!(solved.boxes.contains_key("disconnected"));
        assert_eq!(solved.routes.len(), p.edges.len());
        for edge in &p.edges {
            let route = solved.routes.iter().find(|r| r.edge_id == edge.id).unwrap();
            assert_eq!(route.semantic_ids, edge.semantic_ids);
            assert!(
                solved.boxes[&edge.source]
                    .bounds
                    .contains_point(route.points[0])
            );
            assert!(
                solved.boxes[&edge.target]
                    .bounds
                    .contains_point(*route.points.last().unwrap())
            );
        }
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn node(id: &str, parent: Option<&str>, solid: bool) -> Node {
        Node {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            kind: if solid {
                NodeKind::Service
            } else {
                NodeKind::Scope
            },
            band: 0,
            min_size: Point::new(10, 10, 10),
            preferred_size: Point::new(40, 40, 40),
            max_size: Point::new(60, 60, 60),
            solid,
        }
    }

    fn fixture() -> (Projection, BTreeMap<String, Point>) {
        let projection = Projection {
            key: "size-range-fixture".into(),
            model_id: "size-range-fixture".into(),
            revision: 1,
            root: "root".into(),
            nodes: vec![
                node("root", None, false),
                node("a", Some("root"), true),
                node("b", Some("root"), true),
            ],
            edges: vec![Edge {
                bundle: None,
                id: "call".into(),
                source: "a".into(),
                target: "b".into(),
                semantic_ids: vec!["call".into()],
            }],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        };
        (
            projection,
            BTreeMap::from([
                ("a".into(), Point::ZERO),
                ("b".into(), Point::new(24, 0, 0)),
            ]),
        )
    }

    fn compute(
        projection: &Projection,
        pins: &BTreeMap<String, Point>,
        previous: Option<&LayoutSolution>,
        budget: usize,
        size_candidates: usize,
        cancelled: &AtomicBool,
    ) -> Result<LayoutSolution, LayoutError> {
        solve(LayoutRequest {
            projection,
            algorithm: Algorithm::Floorplan,
            pins,
            previous,
            quality: QualityProfile {
                size_candidates,
                ..Default::default()
            },
            work_budget: budget,
            cancelled,
        })
    }

    #[test]
    fn legal_size_ranges_resolve_pinned_squares_without_uniformly_shrinking_them() {
        let (projection, pins) = fixture();
        let cancelled = AtomicBool::new(false);
        assert!(compute(&projection, &pins, None, 28, 0, &cancelled).is_err());
        let solution = compute(&projection, &pins, None, 28, 28, &cancelled).unwrap();
        validate(&projection, &solution, &pins, 2).unwrap();
        for id in ["a", "b"] {
            assert_eq!(solution.boxes[id].bounds.center(), pins[id]);
            assert_eq!(solution.boxes[id].bounds.size(), Point::new(10, 40, 40));
        }
        assert_eq!(solution.routes.len(), 1);
        assert_eq!(solution.metrics, measure(&projection, &solution, None));
        assert!(
            solution
                .diagnostics
                .iter()
                .any(|message| message.contains("Size-range fallback:"))
        );
        let reused = compute(&projection, &pins, Some(&solution), 28, 0, &cancelled).unwrap();
        assert_eq!(reused.boxes, solution.boxes);
        assert_eq!(reused.routes, solution.routes);
    }

    #[test]
    fn feasible_ordinary_layouts_and_scales_are_unchanged_by_enabling_size_search() {
        let (projection, mut pins) = fixture();
        pins.insert("b".into(), Point::new(100, 0, 0));
        let cancel = AtomicBool::new(false);
        let baseline = compute(&projection, &pins, None, 28, 0, &cancel).unwrap();
        assert_eq!(
            baseline,
            compute(&projection, &pins, None, 28, 28, &cancel).unwrap()
        );
    }

    #[test]
    fn size_fallback_preserves_bands_ordering_and_input_order() {
        let (mut projection, mut pins) = fixture();
        projection
            .nodes
            .iter_mut()
            .find(|node| node.id == "b")
            .unwrap()
            .band = 1;
        pins.insert("b".into(), Point::new(0, 24, 0));
        let cancel = AtomicBool::new(false);
        let solution = compute(&projection, &pins, None, 28, 28, &cancel).unwrap();
        validate(&projection, &solution, &pins, 2).unwrap();
        assert!(solution.boxes["a"].bounds.max.y + 12 <= solution.boxes["b"].bounds.min.y);
        for id in ["a", "b"] {
            assert_eq!(solution.boxes[id].bounds.size(), Point::new(40, 10, 40));
        }
        projection.nodes.reverse();
        projection.edges.reverse();
        assert_eq!(
            solution,
            compute(&projection, &pins, None, 28, 28, &cancel).unwrap()
        );
    }

    #[test]
    fn repair_size_search_preserves_accepted_dimensions_and_unrelated_geometry() {
        let (mut projection, mut pins) = fixture();
        projection.nodes[1].parent = Some("region".into());
        projection.nodes[1].preferred_size = Point::new(20, 40, 40);
        projection.nodes[2].id = "neighbor".into();
        projection.nodes[2].semantic_ids = vec!["neighbor".into()];
        projection.nodes[2].preferred_size = Point::new(20, 30, 40);
        projection.nodes.push(node("region", Some("root"), false));
        projection.edges.clear();
        pins.remove("b");
        pins.insert("neighbor".into(), Point::new(200, 0, 0));
        let cancel = AtomicBool::new(false);
        let previous = compute(&projection, &pins, None, 28, 0, &cancel).unwrap();
        projection.revision += 1;
        projection.key = "size-range-local-addition".into();
        for node in &mut projection.nodes {
            node.preferred_size = Point::new(40, 40, 40);
        }
        projection.nodes.push(node("b", Some("region"), true));
        pins.insert("b".into(), Point::new(24, 0, 0));
        assert!(compute(&projection, &pins, Some(&previous), 28, 0, &cancel).is_err());
        let solution = compute(&projection, &pins, Some(&previous), 28, 28, &cancel).unwrap();
        validate(&projection, &solution, &pins, 2).unwrap();
        assert_eq!(
            solution.boxes["a"].bounds.size(),
            previous.boxes["a"].bounds.size()
        );
        assert_eq!(solution.boxes["neighbor"], previous.boxes["neighbor"]);
        assert!(solution.boxes["b"].bounds.size().x < 40);
    }

    #[test]
    fn fallback_has_an_explicit_small_budget_and_cancellation_publishes_nothing() {
        let (projection, pins) = fixture();
        let cancel = AtomicBool::new(false);
        assert!(compute(&projection, &pins, None, 0, 28, &cancel).is_err());
        let solution = compute(&projection, &pins, None, 1, 28, &cancel).unwrap();
        validate(&projection, &solution, &pins, 2).unwrap();
        assert!(
            solution
                .diagnostics
                .iter()
                .any(|message| message.contains("1 additional candidate evaluations (limit 1)"))
        );
        cancel.store(true, Ordering::Release);
        assert_eq!(
            compute(&projection, &pins, None, 28, 28, &cancel),
            Err(LayoutError::Cancelled)
        );
    }

    #[test]
    fn range_variants_respect_min_max_aspect_and_spanning_thickness() {
        let mut normal = node("ordinary", None, true);
        normal.min_size = Point::new(1, 1, 1);
        normal.max_size = Point::new(400, 400, 400);
        let mut slab = normal.clone();
        slab.kind = NodeKind::Concern;
        slab.preferred_size.y = 4;
        for variant in 0..28 {
            let size = size_variant(&normal, variant);
            assert_eq!(size.min(normal.min_size), normal.min_size);
            assert_eq!(size.max(normal.max_size), normal.max_size);
            assert!(size.x.max(size.y).max(size.z) <= 4 * size.x.min(size.y).min(size.z));
            assert_eq!(size_variant(&slab, variant).y, 4);
        }
    }

    #[test]
    fn exhausted_size_ranges_report_work_without_violating_minimum_dimensions() {
        let (mut projection, pins) = fixture();
        for node in projection.nodes.iter_mut().filter(|node| node.solid) {
            node.min_size.x = 30;
        }
        let error = compute(&projection, &pins, None, 28, 28, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("Size-range fallback:"));
        assert!(error.to_string().contains("limit 28"));
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    #[test]
    fn solver_routes_new_feedback_candidates_beyond_the_static_initializer() {
        let (mut projection, _, _) = obstacle_fixture();
        let obstacle = projection
            .nodes
            .iter_mut()
            .find(|n| n.id == "obstacle")
            .unwrap();
        obstacle.min_size.z = 14;
        obstacle.max_size.z = 14;
        obstacle.preferred_size.z = 14;
        // The static rectangular packing puts the thin obstacle at x=6..26,
        // z=0..14. Pin the endpoints across its centerline. Feedback must discover
        // the short horizontal slide that clears this measured route detour.
        let pins = BTreeMap::from([
            ("source".into(), Point::new(-30, 15, 7)),
            ("target".into(), Point::new(80, 15, 7)),
        ]);
        let cancelled = AtomicBool::new(false);
        let run = |feedback_candidates| {
            solve(LayoutRequest {
                projection: &projection,
                algorithm: Algorithm::Treemap,
                pins: &pins,
                previous: None,
                quality: QualityProfile {
                    feedback_candidates,
                    ..QualityProfile::default()
                },
                work_budget: 32,
                cancelled: &cancelled,
            })
            .unwrap()
        };
        let static_candidate = run(0);
        let refined = run(4);
        assert!(refined.metrics.structural_cost < static_candidate.metrics.structural_cost);
        assert!(refined.metrics.route_length < static_candidate.metrics.route_length);
        assert!(
            refined.metrics.bends < static_candidate.metrics.bends,
            "static {:?} {:?}; refined {:?} {:?}",
            static_candidate.metrics,
            static_candidate.routes,
            refined.metrics,
            refined.routes
        );
        assert_ne!(refined.boxes, static_candidate.boxes);
        assert_eq!(refined.routes.len(), projection.edges.len());
        validate(&projection, &refined, &pins, 2).unwrap();
        assert!(
            static_candidate
                .diagnostics
                .iter()
                .all(|message| !message.contains("Routing feedback"))
        );
        assert!(
            refined
                .diagnostics
                .iter()
                .any(|message| message.contains("Routing feedback"))
        );
    }

    fn obstacle_fixture() -> (Projection, LayoutSolution, BTreeMap<String, Point>) {
        let boxes: BTreeMap<_, _> = [
            (
                "root",
                None,
                false,
                Point::new(-6, -16, -6),
                Point::new(116, 26, 16),
            ),
            (
                "source",
                Some("root"),
                true,
                Point::ZERO,
                Point::new(10, 10, 10),
            ),
            (
                "target",
                Some("root"),
                true,
                Point::new(100, 0, 0),
                Point::new(110, 10, 10),
            ),
            // A thin obstacle blocks the shortest central passage. Sliding it
            // sideways clears that passage without growing the assembly.
            (
                "obstacle",
                Some("root"),
                true,
                Point::new(40, -10, 4),
                Point::new(60, 20, 6),
            ),
        ]
        .into_iter()
        .map(|(id, parent, solid, min, max)| {
            (
                id.to_owned(),
                LayoutBox {
                    id: id.to_owned(),
                    semantic_ids: vec![id.to_owned()],
                    parent: parent.map(str::to_owned),
                    bounds: Bounds { min, max },
                    solid,
                },
            )
        })
        .collect();
        let nodes = boxes
            .values()
            .map(|b| Node {
                id: b.id.clone(),
                semantic_ids: b.semantic_ids.clone(),
                parent: b.parent.clone(),
                kind: if b.solid {
                    NodeKind::Service
                } else {
                    NodeKind::Scope
                },
                band: 0,
                min_size: b.bounds.size(),
                max_size: b.bounds.size(),
                preferred_size: b.bounds.size(),
                solid: b.solid,
            })
            .collect();
        let projection = Projection {
            key: "feedback-fixture".into(),
            model_id: "feedback-fixture".into(),
            revision: 1,
            root: "root".into(),
            nodes,
            edges: vec![Edge {
                bundle: None,
                id: "dependency".into(),
                source: "source".into(),
                target: "target".into(),
                semantic_ids: vec!["dependency".into()],
            }],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        };
        let solution = LayoutSolution {
            model_id: projection.model_id.clone(),
            revision: 1,
            projection_key: projection.key.clone(),
            version: LAYOUT_VERSION,
            boxes,
            routes: vec![],
            metrics: Metrics::default(),
            diagnostics: vec![],
        };
        let pins = BTreeMap::from([
            ("source".into(), solution.boxes["source"].bounds.center()),
            ("target".into(), solution.boxes["target"].bounds.center()),
        ]);
        (projection, solution, pins)
    }

    fn refine(
        projection: &Projection,
        mut initial: LayoutSolution,
        pins: &BTreeMap<String, Point>,
        affected: Option<&BTreeSet<String>>,
        cancel: &AtomicBool,
    ) -> Result<(LayoutSolution, LayoutSolution), LayoutError> {
        let req = LayoutRequest {
            projection,
            algorithm: Algorithm::Floorplan,
            pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 4,
            cancelled: cancel,
        };
        let mut evidence = vec![];
        initial.routes = route_candidate(&initial, &req, &mut evidence)?;
        validate(projection, &initial, pins, 2)?;
        initial.metrics = measure(projection, &initial, None);
        assert!(
            !evidence.is_empty(),
            "actual routing must expose the obstructed shorter passage"
        );
        assert_eq!(evidence[0].edge_id, "dependency");
        assert!(evidence[0].excess_length > 0);
        assert!(
            evidence[0]
                .blocked
                .iter()
                .any(|block| block.obstacle == "obstacle")
        );
        let nodes = projection
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();
        let mut best = Some(initial.clone());
        refine_routing_feedback(
            &req,
            &nodes,
            affected,
            (initial.clone(), evidence),
            &mut best,
            &mut LayoutError::Invalid("unused".into()),
            &mut CandidateBudget::legacy(),
        )?;
        Ok((initial, best.unwrap()))
    }

    #[test]
    fn strict_feedback_budget_charges_a_trial_rejected_before_routing() {
        let (mut projection, mut initial, pins) = obstacle_fixture();
        // The shortest proposed move takes the obstacle into this neighbor.
        // Its geometry is rejected before routing, but it still spends work.
        let guard = LayoutBox {
            id: "guard".into(),
            semantic_ids: vec!["guard".into()],
            parent: Some("root".into()),
            bounds: Bounds {
                min: Point::new(40, -10, 1),
                max: Point::new(60, 20, 3),
            },
            solid: true,
        };
        projection.nodes.push(Node {
            id: guard.id.clone(),
            semantic_ids: guard.semantic_ids.clone(),
            parent: guard.parent.clone(),
            kind: NodeKind::Service,
            band: 0,
            min_size: guard.bounds.size(),
            max_size: guard.bounds.size(),
            preferred_size: guard.bounds.size(),
            solid: true,
        });
        initial.boxes.insert(guard.id.clone(), guard);
        let cancel = AtomicBool::new(false);
        let req = LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 1,
            cancelled: &cancel,
        };
        initial.routes = routing::route_all(&initial.boxes, &projection.edges, 2, &cancel).unwrap();
        validate(&projection, &initial, &pins, 2).unwrap();
        initial.metrics = measure(&projection, &initial, None);
        let evidence = vec![routing::RouteFeedback {
            edge_id: "dependency".into(),
            failed: false,
            excess_length: 10,
            blocked: vec![routing::BlockedSegment {
                obstacle: "obstacle".into(),
                start: Point::new(10, 5, 5),
                end: Point::new(100, 5, 5),
            }],
        }];
        let moves = feedback_moves(&initial.boxes, &evidence, &pins, None, 2);
        assert_eq!(moves[0].1, Point::new(0, 0, -3));
        assert!(
            initial.boxes["obstacle"]
                .bounds
                .translated(moves[0].1)
                .overlaps(initial.boxes["guard"].bounds)
        );
        let nodes = projection
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();
        let mut best = Some(initial.clone());
        let mut budget = CandidateBudget::strict(1);
        refine_routing_feedback(
            &req,
            &nodes,
            None,
            (initial.clone(), evidence),
            &mut best,
            &mut LayoutError::Invalid("unused".into()),
            &mut budget,
        )
        .unwrap();
        assert_eq!(budget.evaluations, 1);
        assert_eq!(budget.feedback, 1);
        assert_eq!(best.unwrap(), initial);
    }

    #[test]
    fn measured_obstacle_detour_opens_a_new_shorter_passage_and_preserves_pins() {
        let (projection, solution, pins) = obstacle_fixture();
        let (before, after) =
            refine(&projection, solution, &pins, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(before.metrics.route_length, 100);
        assert_eq!(after.metrics.route_length, 90);
        assert!(after.metrics.structural_cost < before.metrics.structural_cost);
        assert_ne!(
            after.boxes["obstacle"].bounds,
            before.boxes["obstacle"].bounds
        );
        assert_eq!(after.boxes["source"], before.boxes["source"]);
        assert_eq!(after.boxes["target"], before.boxes["target"]);
        assert_eq!(after.metrics.volume, before.metrics.volume);
        validate(&projection, &after, &pins, 2).unwrap();
        assert!(
            after
                .diagnostics
                .iter()
                .any(|d| d.contains("Routing feedback evaluated")
                    && d.contains("retained 1 validated improvements"))
        );
    }

    #[test]
    fn feedback_respects_pinned_obstacles_and_unaffected_regions() {
        let (projection, solution, mut pins) = obstacle_fixture();
        pins.insert(
            "obstacle".into(),
            solution.boxes["obstacle"].bounds.center(),
        );
        let (before, after) = refine(
            &projection,
            solution.clone(),
            &pins,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(before, after);
        pins.remove("obstacle");
        let affected = BTreeSet::from(["source".to_owned()]);
        let (before, after) = refine(
            &projection,
            solution,
            &pins,
            Some(&affected),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn failed_attachment_escape_supplies_feedback_that_opens_a_route() {
        let (mut projection, mut initial, pins) = obstacle_fixture();
        projection.nodes.retain(|node| node.id != "obstacle");
        initial.boxes.remove("obstacle");
        initial.boxes.get_mut("root").unwrap().bounds = Bounds {
            min: Point::new(-30, -30, -30),
            max: Point::new(120, 40, 40),
        };
        for (id, min, max) in [
            ("left", Point::new(-3, -20, -20), Point::new(-1, 30, 30)),
            ("right", Point::new(11, -20, -20), Point::new(13, 30, 30)),
            ("bottom", Point::new(0, -3, -20), Point::new(10, -1, 30)),
            ("top", Point::new(0, 11, -20), Point::new(10, 13, 30)),
            ("back", Point::new(0, 0, -3), Point::new(10, 10, -1)),
            ("front", Point::new(0, 0, 11), Point::new(10, 10, 13)),
        ] {
            let bounds = Bounds { min, max };
            initial.boxes.insert(
                id.into(),
                LayoutBox {
                    id: id.into(),
                    semantic_ids: vec![id.into()],
                    parent: Some("root".into()),
                    bounds,
                    solid: true,
                },
            );
            projection.nodes.push(Node {
                id: id.into(),
                semantic_ids: vec![id.into()],
                parent: Some("root".into()),
                kind: NodeKind::Service,
                band: 0,
                min_size: bounds.size(),
                max_size: bounds.size(),
                preferred_size: bounds.size(),
                solid: true,
            });
        }
        validation::validate_boxes(&projection, &initial, &pins).unwrap();
        let cancel = AtomicBool::new(false);
        let req = LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 4,
            cancelled: &cancel,
        };
        let mut evidence = vec![];
        let error = route_candidate(&initial, &req, &mut evidence).unwrap_err();
        assert!(matches!(error, LayoutError::Unroutable(_)));
        assert!(
            evidence
                .iter()
                .any(|e| e.failed && e.blocked.iter().any(|b| b.obstacle == "right"))
        );
        let nodes = projection
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();
        let mut best = None;
        refine_routing_feedback(
            &req,
            &nodes,
            None,
            (initial.clone(), evidence),
            &mut best,
            &mut error.clone(),
            &mut CandidateBudget::legacy(),
        )
        .unwrap();
        let after = best.expect("opening the observed blocked escape must make routing possible");
        assert_eq!(after.routes.len(), 1);
        assert_eq!(after.metrics.route_length, 90);
        assert_eq!(after.boxes["source"], initial.boxes["source"]);
        assert_eq!(after.boxes["target"], initial.boxes["target"]);
        validate(&projection, &after, &pins, 2).unwrap();
    }

    #[test]
    fn feedback_is_deterministic_under_input_reordering_and_cancellable() {
        let (mut projection, solution, pins) = obstacle_fixture();
        let (_, first) = refine(
            &projection,
            solution.clone(),
            &pins,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        projection.nodes.reverse();
        projection.edges.reverse();
        let (_, second) = refine(
            &projection,
            solution.clone(),
            &pins,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(first, second);
        assert!(matches!(
            refine(&projection, solution, &pins, None, &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, solid: bool) -> Node {
        let size = Point::new(10, 10, 10);
        Node {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            kind: if solid {
                NodeKind::Service
            } else {
                NodeKind::Scope
            },
            band: 0,
            min_size: size,
            max_size: size,
            preferred_size: size,
            solid,
        }
    }

    fn projection() -> Projection {
        Projection {
            key: "original".into(),
            model_id: "repair-fixture".into(),
            revision: 1,
            root: "root".into(),
            nodes: vec![
                node("root", None, false),
                node("a", Some("root"), false),
                node("b", Some("root"), false),
                node("a1", Some("a"), true),
                node("b1", Some("b"), true),
            ],
            edges: vec![],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        }
    }

    fn compute(
        projection: &Projection,
        pins: &BTreeMap<String, Point>,
        previous: Option<&LayoutSolution>,
    ) -> Result<LayoutSolution, LayoutError> {
        solve(LayoutRequest {
            projection,
            algorithm: Algorithm::Floorplan,
            pins,
            previous,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &AtomicBool::new(false),
        })
    }

    #[test]
    fn local_addition_keeps_unrelated_assembly_and_its_interior_fixed() {
        let mut projection = projection();
        let pins = BTreeMap::from([
            ("a".into(), Point::ZERO),
            ("b".into(), Point::new(100, 0, 0)),
        ]);
        let previous = compute(&projection, &pins, None).unwrap();
        projection.revision = 2;
        projection.key = "added-local-service".into();
        projection.nodes.push(node("a2", Some("a"), true));
        projection.edges.push(Edge {
            bundle: None,
            id: "local-call".into(),
            source: "a1".into(),
            target: "a2".into(),
            semantic_ids: vec!["local-call".into()],
        });
        let repaired = compute(&projection, &pins, Some(&previous)).unwrap();
        assert_eq!(repaired.boxes["b"], previous.boxes["b"]);
        assert_eq!(repaired.boxes["b1"], previous.boxes["b1"]);
        assert_eq!(repaired.routes.len(), 1);
        assert_eq!(repaired.boxes["a"].bounds.center(), Point::ZERO);
        validate(&projection, &repaired, &pins, 2).unwrap();
    }

    #[test]
    fn impossible_local_growth_returns_error_instead_of_moving_a_neighbor() {
        let mut projection = projection();
        let pins = BTreeMap::from([
            ("a".into(), Point::ZERO),
            ("b".into(), Point::new(24, 0, 0)),
        ]);
        let previous = compute(&projection, &pins, None).unwrap();
        let before = previous.boxes.clone();
        projection.revision = 2;
        projection.key = "oversized-local-service".into();
        let mut large = node("large", Some("a"), true);
        large.preferred_size = Point::new(100, 10, 100);
        large.min_size = large.preferred_size;
        large.max_size = large.preferred_size;
        projection.nodes.push(large);
        assert!(compute(&projection, &pins, Some(&previous)).is_err());
        assert_eq!(previous.boxes, before);
    }

    #[test]
    fn changed_relations_reuse_geometry_before_considering_repair() {
        let mut projection = projection();
        let pins = BTreeMap::new();
        let previous = compute(&projection, &pins, None).unwrap();
        projection.revision += 1;
        projection.key = "added-cross-scope-edge".into();
        projection.edges.push(Edge {
            bundle: None,
            id: "dependency".into(),
            source: "a1".into(),
            target: "b1".into(),
            semantic_ids: vec!["dependency".into()],
        });
        let repaired = compute(&projection, &pins, Some(&previous)).unwrap();
        assert_eq!(repaired.boxes, previous.boxes);
        assert_eq!(repaired.metrics.displacement, 0);
        assert_eq!(repaired.routes.len(), 1);
    }

    #[test]
    fn parent_and_child_pins_survive_envelope_refitting() {
        let mut projection = projection();
        projection
            .nodes
            .retain(|n| !["b", "b1"].contains(&n.id.as_str()));
        // Child sorts after parent here; both fixed centers must survive the
        // parent's frame refit, including expansion asymmetrical to the child.
        let pins = BTreeMap::from([
            ("a".into(), Point::new(100, 0, 0)),
            ("a1".into(), Point::new(110, 0, 0)),
        ]);
        let solution = compute(&projection, &pins, None).unwrap();
        for (id, point) in pins {
            assert_eq!(solution.boxes[&id].bounds.center(), point);
        }
    }

    #[test]
    fn odd_dimensions_preserve_pins_across_negative_coordinates() {
        let mut projection = projection();
        let mut odd = node("odd", None, true);
        odd.preferred_size = Point::new(1, 3, 5);
        odd.min_size = odd.preferred_size;
        odd.max_size = odd.preferred_size;
        projection.nodes = vec![odd];
        projection.root = "odd".into();
        for center in [Point::new(-1, -1, -1), Point::ZERO, Point::new(1, 1, 1)] {
            let pins = BTreeMap::from([("odd".into(), center)]);
            let solution = compute(&projection, &pins, None).unwrap();
            let bounds = solution.boxes["odd"].bounds;
            assert_eq!(bounds.center(), center);
            assert_eq!(bounds.size(), Point::new(1, 3, 5));
            for delta in [Point::new(-10, 5, -3), Point::new(7, -8, 11)] {
                assert_eq!(bounds.translated(delta).center(), center + delta);
            }
        }
    }

    #[test]
    fn resizing_an_odd_box_retains_its_negative_center_during_repair() {
        let mut projection = projection();
        let mut odd = node("odd", None, true);
        odd.preferred_size = Point::new(1, 3, 5);
        odd.min_size = odd.preferred_size;
        odd.max_size = odd.preferred_size;
        projection.nodes = vec![odd];
        projection.root = "odd".into();
        let center = Point::new(-1, -1, -1);
        let previous =
            compute(&projection, &BTreeMap::from([("odd".into(), center)]), None).unwrap();
        projection.revision += 1;
        let odd = &mut projection.nodes[0];
        odd.preferred_size = Point::new(3, 5, 7);
        odd.min_size = odd.preferred_size;
        odd.max_size = odd.preferred_size;
        let repaired = solve(LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: Some(&previous),
            quality: QualityProfile::default(),
            // The first repair candidate anchors the region by its center.
            work_budget: 1,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        assert_eq!(repaired.boxes["odd"].bounds.center(), center);
        assert_eq!(repaired.boxes["odd"].bounds.size(), Point::new(3, 5, 7));
    }

    #[test]
    fn foreign_previous_layouts_and_extreme_inputs_are_rejected() {
        let mut projection = projection();
        let pins = BTreeMap::new();
        let previous = compute(&projection, &pins, None).unwrap();
        let mut foreign = previous.clone();
        foreign.model_id = "another-model".into();
        assert!(compute(&projection, &pins, Some(&foreign)).is_err());
        foreign = previous.clone();
        foreign.version += 1;
        assert!(compute(&projection, &pins, Some(&foreign)).is_err());
        foreign = previous.clone();
        foreign.boxes.get_mut("a1").unwrap().bounds.min.x = i64::MIN;
        assert!(compute(&projection, &pins, Some(&foreign)).is_err());
        assert!(
            compute(
                &projection,
                &BTreeMap::from([("a".into(), Point::new(i64::MIN, 0, 0))]),
                None
            )
            .is_err()
        );
        projection.nodes[0].band = i32::MIN;
        assert!(compute(&projection, &pins, None).is_err());
    }

    #[test]
    fn responsibility_bands_align_single_target_auth_without_bends() {
        let mut projection = projection();
        projection.nodes.retain(|n| n.id == "root");
        let mut auth = node("auth", Some("root"), true);
        auth.kind = NodeKind::Concern;
        auth.band = 2;
        let mut middleware = node("middleware", Some("root"), true);
        middleware.band = 1;
        projection.nodes.extend([auth, middleware]);
        projection.edges.push(Edge {
            bundle: None,
            id: "coverage".into(),
            source: "auth".into(),
            target: "middleware".into(),
            semantic_ids: vec!["coverage".into()],
        });
        let solution = compute(&projection, &BTreeMap::new(), None).unwrap();
        let route = &solution.routes[0].points;
        assert_eq!(route.len(), 2);
        assert_eq!(route[0].x, route[1].x);
        assert_eq!(route[0].z, route[1].z);
        assert!(route[0].y > route[1].y);
    }
}
