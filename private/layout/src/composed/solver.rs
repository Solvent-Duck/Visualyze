use super::*;
use crate::{
    Bounds, LayoutRequest, LayoutSolution, QUALITY_VERSION,
    assembly::{AssemblyInstance, CanonicalFrame, DyadicTransform, MAX_DEPTH, MAX_FRAMES},
    assembly_plan::{EndpointPlan, FramePlan, plan_frames},
    check_cancel,
    routing::{FragmentTerminal, RouteFeedback, route_fragments_with_feedback},
    solver::solve_budgeted_with_bounds,
};
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;

/// Place bounded interiors, fit their presentation envelopes, and route each
/// original relationship through its complete frame path. No partial result is
/// published if placement, boundary routing, pins or final validation fail.
/// This entry point has no implicit flat-layout/pin migration or persistent cache.
pub fn solve_composed(req: ComposedRequest<'_>) -> Result<ComposedSolution, LayoutError> {
    solve_prepared(req, None)
}

pub(super) fn solve_prepared(
    req: ComposedRequest<'_>,
    prepared: Option<&super::cache::Prepared<'_>>,
) -> Result<ComposedSolution, LayoutError> {
    check_cancel(req.cancelled)?;
    let plan = plan_frames(req.projection, req.cancelled)?;
    super::validation::check_plan_capacity(&plan, req.cancelled)?;
    let order = frame_order(&plan)?;
    let mut misses = order.len() - prepared.map_or(0, |prepared| prepared.reused.len());
    if req.work_budget < misses {
        return Err(invalid("work budget cannot evaluate every canonical frame"));
    }
    for (frame, pins) in req.pins {
        let kernel = plan
            .frames
            .get(frame)
            .ok_or_else(|| invalid("unknown pin frame"))?;
        if pins
            .keys()
            .any(|id| !kernel.projection.nodes.iter().any(|n| &n.id == id))
        {
            return Err(invalid("pin object is absent from its canonical frame"));
        }
    }
    // Only new interiors may move during ordinary navigation/repair. Explicit
    // reorganization may search free frames; reused locked frames stay fixed.
    let eligible: BTreeSet<_> = plan
        .frames
        .keys()
        .filter(|id| {
            prepared.is_none_or(|prepared| {
                !prepared.reused.contains_key(*id)
                    && (prepared.search
                        || prepared
                            .previous
                            .is_none_or(|old| old.assembly.frame(id).is_err()))
            })
        })
        .cloned()
        .collect();
    let has_boundary_work = plan.edges.values().any(|edge| {
        [&edge.source, &edge.target].into_iter().any(|end| {
            end.steps
                .iter()
                .skip(usize::from(end.surface_endpoint))
                .any(|step| eligible.contains(&step.frame_id))
        })
    });
    // Reserve within the same shared allowance, never add trials on top of it.
    // The profile is bounded just as in the flat solver, even on all-cache hits.
    if req.quality.feedback_candidates > 16 {
        return Err(invalid(
            "feedback candidate limit exceeds supported maximum",
        ));
    }
    let feedback_allowance = if has_boundary_work {
        req.quality
            .feedback_candidates
            .min(req.work_budget.saturating_sub(misses))
    } else {
        0
    };
    let no_pins = BTreeMap::new();
    let no_bounds = BTreeMap::new();
    let mut work = ComposedWork {
        budget: req.work_budget,
        ..Default::default()
    };
    let mut remaining = req.work_budget - feedback_allowance;
    let mut kernels = BTreeMap::new();
    let mut domains = BTreeMap::new();
    // Interior placement does not grow its parent's proxy. Unused evaluations
    // return to the common pool, while every unvisited frame keeps one or more.
    for id in order.iter().rev() {
        check_cancel(req.cancelled)?;
        if let Some((placement, source)) = prepared.and_then(|prepared| prepared.reused.get(id)) {
            work.allocated.insert(id.clone(), 0);
            work.evaluations.insert(id.clone(), 0);
            work.reused.insert(id.clone(), *source);
            kernels.insert(id.clone(), placement.local.clone());
            domains.insert(id.clone(), placement.domain);
            continue;
        }
        let allocation = remaining / misses;
        misses -= 1;
        let previous = prepared
            .and_then(|prepared| prepared.previous)
            .and_then(|previous| previous.assembly.frame(id).ok());
        let result = solve_budgeted_with_bounds(
            LayoutRequest {
                projection: &plan.frames[id].projection,
                algorithm: req.algorithm,
                pins: req.pins.get(id).unwrap_or(&no_pins),
                previous: if prepared.is_some_and(|prepared| prepared.search) {
                    None
                } else {
                    previous.map(CanonicalFrame::solution)
                },
                quality: req.quality,
                work_budget: allocation,
                cancelled: req.cancelled,
            },
            prepared
                .and_then(|prepared| prepared.host_bounds.get(id))
                .unwrap_or(&no_bounds),
        )
        .map_err(|error| frame_error(id, error))?;
        work.allocated.insert(id.clone(), allocation);
        work.evaluations.insert(id.clone(), result.evaluations);
        remaining = remaining
            .checked_sub(result.evaluations)
            .ok_or_else(|| invalid("candidate accounting exceeded common budget"))?;
        let mut solution = result.solution;
        let locked = prepared.and_then(|prepared| prepared.locks.get(id));
        let locked_domain = locked.is_some();
        if let Some(link) = locked
            && solution
                .boxes
                .iter()
                .filter(|(object, _)| *object != id)
                .any(|(_, object)| {
                    !link.canonical_bounds.contains_point(object.bounds.min)
                        || !link.canonical_bounds.contains_point(object.bounds.max)
                })
        {
            return Err(frame_error(
                id,
                invalid(
                    "Anchor-frame pin conflicts: placement cannot fit the locked canonical domain",
                ),
            ));
        }
        let retained = previous.filter(|previous| {
            (!prepared.is_some_and(|prepared| prepared.search) || locked_domain)
                && placement_fits(&solution, previous.bounds())
        });
        let mut domain = if let Some(link) = locked {
            link.canonical_bounds
        } else if let Some(previous) = retained {
            previous.bounds()
        } else {
            routing_domain(&solution, req.quality.clearance.max(1))?
        };
        // A root forest has its own view domain; enlarging one semantic root
        // over parentless references would invent containment and overlap.
        if !solution.boxes[id].solid
            && solution
                .boxes
                .values()
                .filter(|b| b.parent.is_none())
                .count()
                == 1
        {
            if let Some(center) = req.pins.get(id).and_then(|pins| pins.get(id))
                && domain.center() != *center
            {
                if locked_domain {
                    return Err(frame_error(
                        id,
                        invalid("canonical root pin conflicts with locked domain"),
                    ));
                }
                let extent = (*center - domain.min).max(domain.max - *center);
                domain = Bounds {
                    min: *center - extent,
                    max: *center + extent,
                };
            }
            solution.boxes.get_mut(id).unwrap().bounds = domain;
        }
        // These initializer routes are not the final paths. Fixed boundary
        // routing below reroutes local and cross-frame paths together.
        solution.routes.clear();
        solution.metrics = Metrics::default();
        domains.insert(id.clone(), domain);
        kernels.insert(id.clone(), solution);
    }

    let mut instances = vec![AssemblyInstance::root(&plan.root)];
    for id in &order {
        check_cancel(req.cancelled)?;
        let Some(parent) = &plan.frames[id].parent_frame else {
            continue;
        };
        let host = kernels[parent].boxes[id].bounds;
        if let Some(link) = prepared.and_then(|prepared| prepared.locks.get(id)) {
            if link.parent != *parent
                || link.envelope != host
                || link.canonical_bounds != domains[id]
            {
                return Err(frame_error(
                    id,
                    invalid("Anchor-frame pin conflicts with locked host/domain"),
                ));
            }
            instances.push(AssemblyInstance {
                frame_id: id.clone(),
                parent: Some(parent.clone()),
                transform: link.transform,
            });
            continue;
        }
        // A local repair first keeps its accepted transform, domain and host.
        // Anchor pins validate the whole chain after routing; an impossible
        // locked expansion fails instead of silently rescaling the scene.
        if prepared.is_some_and(|prepared| !prepared.search || prepared.locked_domains.contains(id))
            && let Some(previous) = prepared.and_then(|prepared| prepared.previous)
            && let (Ok(frame), Ok(instance), Ok(Some(envelope))) = (
                previous.assembly.frame(id),
                previous.assembly.instance(id),
                previous.assembly.parent_envelope(id),
            )
            && instance.parent.as_ref() == Some(parent)
            && envelope == host
            && frame.bounds().contains_point(domains[id].min)
            && frame.bounds().contains_point(domains[id].max)
        {
            domains.insert(id.clone(), frame.bounds());
            kernels
                .get_mut(id)
                .unwrap()
                .boxes
                .get_mut(id)
                .unwrap()
                .bounds = frame.bounds();
            instances.push(instance.clone());
            continue;
        }
        let mut fit = DyadicTransform::fit_envelope(domains[id], host).map_err(assembly_error)?;
        // Distribute presentation padding around the existing interior. Keeping
        // its center prevents a single nested dependency drifting into a corner
        // and requiring bends solely because its envelope is larger.
        let center = domains[id].center();
        let size = fit.canonical_bounds.size();
        let half = Point::new(size.x / 2, size.y / 2, size.z / 2);
        fit.canonical_bounds = Bounds {
            min: center - half,
            max: center - half + size,
        };
        fit.transform = DyadicTransform::new(
            fit.canonical_bounds.min,
            host.min,
            fit.transform.scale_shift(),
        )
        .map_err(assembly_error)?;
        domains.insert(id.clone(), fit.canonical_bounds);
        kernels
            .get_mut(id)
            .unwrap()
            .boxes
            .get_mut(id)
            .unwrap()
            .bounds = fit.canonical_bounds;
        instances.push(AssemblyInstance::child(id, parent, fit.transform));
    }
    let frames = kernels
        .into_iter()
        .map(|(id, solution)| {
            CanonicalFrame::with_root(id.clone(), domains[&id], solution).map_err(assembly_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    // This also checks total composed scale/depth before any boundary routing.
    let assembly = Assembly::new(frames, instances).map_err(assembly_error)?;
    let initial = route_or_recover(
        req,
        &plan,
        &order,
        assembly,
        &mut work,
        &eligible,
        feedback_allowance,
        prepared,
    )?;
    refine_composed(
        req,
        &plan,
        &order,
        initial,
        &mut work,
        &eligible,
        feedback_allowance,
        prepared,
    )
}

/// Evidence belongs to one actually failed routing batch. Partial paths never
/// become a solution; only their observed obstacle corridors guide a new trial.
struct FrameRoutingFailure {
    frame: String,
    evidence: Vec<RouteFeedback>,
}

impl FrameRoutingFailure {
    fn progress(&self, order: &[String]) -> (usize, &str, std::cmp::Reverse<usize>) {
        // Moving past an earlier failure, or reducing its distinct blockers,
        // permits a bounded sequence of repairs without accepting partial routes.
        let frame = order.iter().position(|id| id == &self.frame).unwrap();
        let edge = self.evidence.first().map_or("", |f| f.edge_id.as_str());
        let blockers = self
            .evidence
            .iter()
            .flat_map(|f| f.blocked.iter().map(|b| b.obstacle.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        (frame, edge, std::cmp::Reverse(blockers))
    }
}

#[allow(clippy::too_many_arguments)]
fn route_or_recover(
    req: ComposedRequest<'_>,
    plan: &FramePlan,
    order: &[String],
    mut seed: Assembly,
    work: &mut ComposedWork,
    eligible: &BTreeSet<String>,
    allowance: usize,
    prepared: Option<&super::cache::Prepared<'_>>,
) -> Result<ComposedSolution, LayoutError> {
    if allowance == 0 || eligible.is_empty() {
        return route_assembly(req, plan, order, seed, work, prepared, &mut None);
    }
    let mut failure = None;
    let initial_error =
        match route_assembly(req, plan, order, seed.clone(), work, prepared, &mut failure) {
            Ok(solution) => return Ok(solution),
            Err(error @ LayoutError::Unroutable(_)) => error,
            Err(error) => return Err(error),
        };
    let no_hosts = BTreeMap::new();
    let hosts = prepared.map_or(&no_hosts, |p| &p.host_bounds);
    let mut tried = BTreeSet::new();
    while work.feedback_evaluations < allowance {
        check_cancel(req.cancelled)?;
        let Some(observed) = &failure else { break };
        let proposals = super::feedback::failure_moves(
            &seed,
            &observed.frame,
            &observed.evidence,
            eligible,
            req.pins,
            hosts,
            req.quality.clearance,
            64,
            req.cancelled,
        )?;
        let Some(movement) = proposals.into_iter().find(|movement| {
            let center = seed.frame(&movement.frame).unwrap().solution().boxes[&movement.object]
                .bounds
                .center();
            let target = center + movement.delta;
            !tried.contains(&(
                movement.frame.clone(),
                movement.object.clone(),
                target.x,
                target.y,
                target.z,
            ))
        }) else {
            break;
        };
        let target = seed.frame(&movement.frame).unwrap().solution().boxes[&movement.object]
            .bounds
            .center()
            + movement.delta;
        work.feedback_evaluations += 1;
        tried.insert((
            movement.frame.clone(),
            movement.object.clone(),
            target.x,
            target.y,
            target.z,
        ));
        let mut next_failure = None;
        let candidate = (|| {
            let assembly = super::feedback::apply(&seed, &movement, req.cancelled)?;
            let empty = BTreeMap::new();
            crate::validation::validate_boxes(
                &plan.frames[&movement.frame].projection,
                assembly
                    .frame(&movement.frame)
                    .map_err(assembly_error)?
                    .solution(),
                req.pins.get(&movement.frame).unwrap_or(&empty),
            )?;
            let routed = route_assembly(
                req,
                plan,
                order,
                assembly.clone(),
                work,
                prepared,
                &mut next_failure,
            );
            Ok((assembly, routed))
        })();
        match candidate {
            Ok((_, Ok(mut solution))) => {
                // Feasibility is the first quality gate. Later refinement uses
                // only the remaining portion of the original allowance.
                work.feedback_improvements += 1;
                solution.work = work.clone();
                return Ok(solution);
            }
            Ok((_, Err(LayoutError::Cancelled))) | Err(LayoutError::Cancelled) => {
                return Err(LayoutError::Cancelled);
            }
            Ok((assembly, Err(LayoutError::Unroutable(_)))) => {
                if let Some(next) = next_failure
                    && next.progress(order) > observed.progress(order)
                {
                    seed = assembly;
                    failure = Some(next);
                }
            }
            Ok((_, Err(_))) | Err(_) => {}
        }
    }
    check_cancel(req.cancelled)?;
    // Preserve the original named frame/relationship diagnostic. A failed
    // search never exports its partial geometry or writes the placement cache.
    Err(initial_error)
}

#[allow(clippy::too_many_arguments)]
fn refine_composed(
    req: ComposedRequest<'_>,
    plan: &FramePlan,
    order: &[String],
    mut best: ComposedSolution,
    work: &mut ComposedWork,
    eligible: &BTreeSet<String>,
    allowance: usize,
    prepared: Option<&super::cache::Prepared<'_>>,
) -> Result<ComposedSolution, LayoutError> {
    let no_hosts = BTreeMap::new();
    let hosts = prepared.map_or(&no_hosts, |prepared| &prepared.host_bounds);
    let mut tried = BTreeSet::new();
    while work.feedback_evaluations < allowance {
        check_cancel(req.cancelled)?;
        let moves = super::feedback::moves(
            &best,
            eligible,
            req.pins,
            hosts,
            req.quality.clearance,
            64,
            req.cancelled,
        )?;
        let Some(movement) = moves.into_iter().find(|m| {
            // Quantized destination keys avoid retrying the same geometry after
            // an accepted move changes its relative delta.
            let center = best.assembly.frame(&m.frame).unwrap().solution().boxes[&m.object]
                .bounds
                .center();
            let target = center + m.delta;
            !tried.contains(&(
                m.frame.clone(),
                m.object.clone(),
                target.x,
                target.y,
                target.z,
            ))
        }) else {
            break;
        };
        let center = best
            .assembly
            .frame(&movement.frame)
            .map_err(assembly_error)?
            .solution()
            .boxes[&movement.object]
            .bounds
            .center();
        let target = center + movement.delta;
        // Charge before constructing or validating geometry, including failures.
        work.feedback_evaluations += 1;
        tried.insert((
            movement.frame.clone(),
            movement.object.clone(),
            target.x,
            target.y,
            target.z,
        ));
        let trial = (|| {
            let assembly = super::feedback::apply(&best.assembly, &movement, req.cancelled)?;
            let empty = BTreeMap::new();
            crate::validation::validate_boxes(
                &plan.frames[&movement.frame].projection,
                assembly
                    .frame(&movement.frame)
                    .map_err(assembly_error)?
                    .solution(),
                req.pins.get(&movement.frame).unwrap_or(&empty),
            )?;
            // The entire route set is rebuilt top-down, including boundaries
            // changed by moving a proxy. Exact matching batches may be reused.
            route_assembly(req, plan, order, assembly, work, prepared, &mut None)
        })();
        match trial {
            Ok(candidate)
                if candidate.metrics.structural_cost < best.metrics.structural_cost
                    && candidate.metrics.canonical_route_length
                        <= best.metrics.canonical_route_length =>
            {
                work.feedback_improvements += 1;
                best = candidate;
            }
            Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
            Ok(_) | Err(_) => {}
        }
    }
    best.work = work.clone();
    validate_composed(req.projection, &best, req.pins, req.cancelled)?;
    check_cancel(req.cancelled)?;
    Ok(best)
}

fn route_assembly(
    req: ComposedRequest<'_>,
    plan: &FramePlan,
    order: &[String],
    assembly: Assembly,
    work: &mut ComposedWork,
    prepared: Option<&super::cache::Prepared<'_>>,
    failure: &mut Option<FrameRoutingFailure>,
) -> Result<ComposedSolution, LayoutError> {
    *failure = None;
    let mut routed: BTreeMap<String, BTreeMap<String, FrameFragment>> = BTreeMap::new();
    let mut point_count = 0usize;
    for id in order {
        check_cancel(req.cancelled)?;
        let mut requests = Vec::new();
        for edge in plan.edges.values() {
            check_cancel(req.cancelled)?;
            let terminals = if edge.lca_frame == *id {
                Some((
                    FragmentTerminal::Box(edge.source.representative.clone()),
                    FragmentTerminal::Box(edge.target.representative.clone()),
                ))
            } else if let Some(index) = participating_step(&edge.source, id) {
                let boundary = boundary_terminal(
                    &edge.source,
                    index,
                    &edge.original.id,
                    true,
                    &routed,
                    &assembly,
                )?;
                Some((
                    FragmentTerminal::Box(inner_box(&edge.source, index)),
                    boundary,
                ))
            } else if let Some(index) = participating_step(&edge.target, id) {
                let boundary = boundary_terminal(
                    &edge.target,
                    index,
                    &edge.original.id,
                    false,
                    &routed,
                    &assembly,
                )?;
                Some((
                    boundary,
                    FragmentTerminal::Box(inner_box(&edge.target, index)),
                ))
            } else {
                None
            };
            if let Some((source, target)) = terminals {
                requests.push(FragmentRequest {
                    id: edge.original.id.clone(),
                    semantic_ids: edge.original.semantic_ids.clone(),
                    source,
                    target,
                });
            }
        }
        let frame = assembly.frame(id).map_err(assembly_error)?;
        let cached = prepared.and_then(|prepared| {
            prepared.routing_cache.routes(
                &req.projection.model_id,
                id,
                frame,
                &requests,
                req.quality.clearance,
            )
        });
        let fragments = if let Some(fragments) = cached {
            work.cache.routing_hits += 1;
            fragments
        } else {
            work.cache.routing_misses += 1;
            let mut evidence = Vec::new();
            let result = route_fragments_with_feedback(
                &frame.solution().boxes,
                frame.bounds(),
                &requests,
                req.quality.clearance,
                req.cancelled,
                &mut evidence,
            );
            if let Err(error) = result {
                if matches!(error, LayoutError::Unroutable(_)) && !evidence.is_empty() {
                    *failure = Some(FrameRoutingFailure {
                        frame: id.clone(),
                        evidence,
                    });
                }
                return Err(frame_error(id, error));
            }
            result.unwrap()
        };
        point_count = fragments.iter().fold(point_count, |count, route| {
            count.saturating_add(route.points.len())
        });
        if point_count > crate::assembly::MAX_TOTAL_ROUTE_POINTS {
            return Err(invalid("composed route vertices exceed budget"));
        }
        let mut by_id: BTreeMap<_, _> = requests.into_iter().map(|r| (r.id.clone(), r)).collect();
        routed.insert(
            id.clone(),
            fragments
                .into_iter()
                .map(|route| {
                    let request = by_id
                        .remove(&route.id)
                        .expect("validated fragment identity");
                    (
                        route.id.clone(),
                        FrameFragment {
                            frame_id: id.clone(),
                            request,
                            route,
                        },
                    )
                })
                .collect(),
        );
    }
    let mut routes = BTreeMap::new();
    for edge in plan.edges.values() {
        check_cancel(req.cancelled)?;
        let source_frames = edge
            .source
            .steps
            .iter()
            .skip(usize::from(edge.source.surface_endpoint));
        let target_frames = edge
            .target
            .steps
            .iter()
            .skip(usize::from(edge.target.surface_endpoint))
            .rev();
        let ids = source_frames
            .map(|step| step.frame_id.as_str())
            .chain(std::iter::once(edge.lca_frame.as_str()))
            .chain(target_frames.map(|step| step.frame_id.as_str()));
        let fragments = ids
            .map(|id| {
                routed
                    .get_mut(id)
                    .and_then(|batch| batch.remove(&edge.original.id))
                    .ok_or_else(|| {
                        invalid(format!("missing fragment for {} in {id}", edge.original.id))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        routes.insert(
            edge.original.id.clone(),
            ComposedRoute {
                edge: edge.original.clone(),
                fragments,
            },
        );
    }
    let mut solution = ComposedSolution {
        model_id: req.projection.model_id.clone(),
        revision: req.projection.revision,
        projection_key: req.projection.key.clone(),
        version: COMPOSITION_VERSION,
        quality_version: QUALITY_VERSION,
        clearance: req.quality.clearance,
        assembly,
        routes,
        metrics: Default::default(),
        work: work.clone(),
    };
    validate_composed(req.projection, &solution, req.pins, req.cancelled)?;
    solution.metrics = super::presentation::measure_validated(plan, &solution, req.cancelled)?;
    check_cancel(req.cancelled)?;
    Ok(solution)
}

fn frame_error(id: &str, error: LayoutError) -> LayoutError {
    match error {
        LayoutError::Invalid(message) => invalid(format!("frame {id}: {message}")),
        LayoutError::Unroutable(message) => {
            LayoutError::Unroutable(format!("frame {id}: {message}"))
        }
        LayoutError::Cancelled => LayoutError::Cancelled,
    }
}

fn frame_order(plan: &FramePlan) -> Result<Vec<String>, LayoutError> {
    if plan.frames.len() > MAX_FRAMES {
        return Err(invalid("frame count exceeds presentation limit"));
    }
    let mut order = vec![plan.root.clone()];
    let mut depths = BTreeMap::from([(plan.root.clone(), 0)]);
    let mut next = 0;
    while next < order.len() {
        let id = &order[next];
        let depth = depths[id];
        for child in &plan.frames[id].child_frames {
            if depth >= MAX_DEPTH {
                return Err(invalid("frame depth exceeds presentation limit"));
            }
            if depths.insert(child.clone(), depth + 1).is_some() {
                return Err(invalid("duplicate frame ownership"));
            }
            order.push(child.clone());
        }
        next += 1;
    }
    if order.len() != plan.frames.len() {
        return Err(invalid("disconnected frame topology"));
    }
    Ok(order)
}

fn placement_fits(solution: &LayoutSolution, domain: Bounds) -> bool {
    solution.boxes.values().all(|object| {
        domain.contains_point(object.bounds.min) && domain.contains_point(object.bounds.max)
    }) && solution
        .routes
        .iter()
        .flat_map(|route| &route.points)
        .all(|point| domain.contains_point(*point))
}

fn routing_domain(solution: &LayoutSolution, padding: i64) -> Result<Bounds, LayoutError> {
    let mut domain = solution
        .boxes
        .values()
        .map(|b| b.bounds)
        .reduce(|a, b| Bounds {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })
        .ok_or_else(|| invalid("empty canonical frame"))?;
    for point in solution.routes.iter().flat_map(|r| &r.points) {
        domain.min = domain.min.min(*point);
        domain.max = domain.max.max(*point);
    }
    // Ordinary kernel validation has already bounded these coordinates.
    domain = domain.inflated(padding);
    Ok(domain)
}

fn participating_step(endpoint: &EndpointPlan, frame: &str) -> Option<usize> {
    endpoint
        .steps
        .iter()
        .enumerate()
        .skip(usize::from(endpoint.surface_endpoint))
        .find(|(_, step)| step.frame_id == frame)
        .map(|(index, _)| index)
}

fn inner_box(endpoint: &EndpointPlan, index: usize) -> String {
    if index == 0 {
        endpoint.node_id.clone()
    } else {
        endpoint.steps[index - 1].proxy_id.clone()
    }
}

fn boundary_terminal(
    endpoint: &EndpointPlan,
    index: usize,
    edge_id: &str,
    source: bool,
    routed: &BTreeMap<String, BTreeMap<String, FrameFragment>>,
    assembly: &Assembly,
) -> Result<FragmentTerminal, LayoutError> {
    let step = &endpoint.steps[index];
    let parent = routed
        .get(&step.parent_frame)
        .and_then(|batch| batch.get(edge_id))
        .ok_or_else(|| invalid("parent boundary route is missing"))?;
    let points = &parent.route.points;
    if points.len() < 2 {
        return Err(invalid("parent route lacks a normal segment"));
    }
    let (attachment, adjacent) = if source {
        (points[0], points[1])
    } else {
        (points[points.len() - 1], points[points.len() - 2])
    };
    let inward = Point::new(
        (attachment.x - adjacent.x).signum(),
        (attachment.y - adjacent.y).signum(),
        (attachment.z - adjacent.z).signum(),
    );
    let child = assembly.frame(&step.frame_id).map_err(assembly_error)?;
    let transform = assembly
        .instance(&step.frame_id)
        .map_err(assembly_error)?
        .transform;
    let point = transform
        .inverse_boundary_terminal(attachment, child.bounds())
        .map_err(assembly_error)?;
    Ok(FragmentTerminal::Boundary { point, inward })
}
