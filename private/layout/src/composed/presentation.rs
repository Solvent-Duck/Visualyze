use super::*;
use crate::{
    Route,
    assembly::{ExactPoint, PresentedScopePort},
    assembly_plan::FramePlan,
    check_cancel, measure,
};
use std::collections::BTreeSet;

impl ComposedSolution {
    /// Present a validated composition in its root frame. Every display object
    /// appears once: a parent's open host supplies the child's envelope, while
    /// the duplicate canonical root is omitted. No coordinates are rounded.
    /// Call `validate_composed` after deserialization or any external mutation.
    pub fn present(&self, cancelled: &AtomicBool) -> Result<ComposedPresentation, LayoutError> {
        let mut boxes = BTreeMap::new();
        for frame_id in self.assembly.frame_ids() {
            check_cancel(cancelled)?;
            let frame = self
                .assembly
                .present(frame_id, self.assembly.root())
                .map_err(assembly_error)?;
            for (id, object) in frame.boxes {
                if id == frame_id && frame_id != self.assembly.root() {
                    continue;
                }
                if boxes.insert(id, object).is_some() {
                    return Err(invalid("duplicate presented object"));
                }
            }
        }
        let mut routes = Vec::with_capacity(self.routes.len());
        for route in self.routes.values() {
            check_cancel(cancelled)?;
            let mut points: Vec<ExactPoint> = Vec::new();
            let mut scope_ports = Vec::new();
            let mut previous_frame: Option<&str> = None;
            for fragment in &route.fragments {
                check_cancel(cancelled)?;
                let map = self
                    .assembly
                    .transform_to(&fragment.frame_id, self.assembly.root())
                    .map_err(assembly_error)?;
                let mapped = fragment
                    .route
                    .points
                    .iter()
                    .map(|p| map.map_point(*p).map_err(assembly_error))
                    .collect::<Result<Vec<_>, _>>()?;
                if mapped.len() < 2 {
                    return Err(invalid("fragment lacks a presented segment"));
                }
                if let Some(previous_frame) = previous_frame {
                    if points.last() != mapped.first() {
                        return Err(invalid("disconnected presented boundary join"));
                    }
                    let (assembly_id, entering) = if self
                        .assembly
                        .instance(previous_frame)
                        .map_err(assembly_error)?
                        .parent
                        .as_deref()
                        == Some(&fragment.frame_id)
                    {
                        (previous_frame.to_owned(), false)
                    } else if self
                        .assembly
                        .instance(&fragment.frame_id)
                        .map_err(assembly_error)?
                        .parent
                        .as_deref()
                        == Some(previous_frame)
                    {
                        (fragment.frame_id.clone(), true)
                    } else {
                        return Err(invalid("nonadjacent presented frames"));
                    };
                    scope_ports.push(PresentedScopePort {
                        assembly_id,
                        point: mapped[0],
                        entering,
                    });
                    points.extend(mapped.into_iter().skip(1));
                } else {
                    points.extend(mapped);
                }
                previous_frame = Some(&fragment.frame_id);
            }
            routes.push(PresentedRoute {
                edge_id: route.edge.id.clone(),
                semantic_ids: route.edge.semantic_ids.clone(),
                points,
                scope_ports,
            });
        }
        check_cancel(cancelled)?;
        Ok(ComposedPresentation { boxes, routes })
    }
}

/// Only called after the independent whole-composition geometry validator.
/// Normalizers are derived from the original allowed geometry and fixed frame
/// partition, never from candidate scales or a winning dimension vector.
pub(super) fn measure_validated(
    plan: &FramePlan,
    solution: &ComposedSolution,
    cancelled: &AtomicBool,
) -> Result<ComposedMetrics, LayoutError> {
    let mut metrics = ComposedMetrics::default();
    for (id, kernel) in &plan.frames {
        check_cancel(cancelled)?;
        let frame = solution.assembly.frame(id).map_err(assembly_error)?;
        let mut local = frame.solution().clone();
        // A bundle's shared trunk is stored on every member: measure it once.
        let mut measured_ids = BTreeSet::new();
        local.routes = solution
            .routes
            .values()
            .flat_map(|route| &route.fragments)
            .filter(|fragment| fragment.frame_id == *id)
            .filter(|fragment| measured_ids.insert(fragment.request.id.clone()))
            .map(|fragment| Route {
                edge_id: fragment.route.id.clone(),
                semantic_ids: fragment.route.semantic_ids.clone(),
                points: fragment.route.points.clone(),
                scope_ports: vec![],
            })
            .collect();
        let mut measured = measure(&kernel.projection, &local, None);
        let size = frame.bounds().size();
        let padded_volume = size.x as f64 * size.y as f64 * size.z as f64;
        let padded_aspect =
            size.x.max(size.y).max(size.z) as f64 / size.x.min(size.y).min(size.z) as f64;
        // A solid focus/reference forest has an independent domain. Include
        // that routing reserve without inventing a semantic box to measure it.
        let reference_volume = kernel
            .projection
            .nodes
            .iter()
            .filter(|n| n.solid)
            .map(|n| {
                n.preferred_size.x as f64 * n.preferred_size.y as f64 * n.preferred_size.z as f64
            })
            .sum::<f64>()
            .max(1.0);
        let weights = crate::quality::SCORING_WEIGHTS;
        measured.structural_cost += ((padded_volume / reference_volume).cbrt()
            - (measured.volume / reference_volume).cbrt())
            * weights.compactness
            + (padded_aspect - measured.aspect_ratio) * weights.aspect_ratio;
        measured.volume = padded_volume;
        measured.aspect_ratio = padded_aspect;
        // Boundary fragments count in the fixed congestion normalization too.
        // Their number is fixed by the original frame topology, not by routing.
        let original_crossing_cost =
            measured.crossing_points as f64 / kernel.projection.edges.len().max(1) as f64;
        let crossing_cost = measured.crossing_points as f64 / local.routes.len().max(1) as f64;
        measured.congestion += crossing_cost - original_crossing_cost;
        measured.structural_cost += (crossing_cost - original_crossing_cost) * weights.congestion;
        metrics.canonical_route_length = metrics
            .canonical_route_length
            .checked_add(measured.route_length)
            .ok_or_else(|| invalid("canonical route length overflow"))?;
        metrics.bends += measured.bends;
        metrics.structural_cost += measured.structural_cost;
        let map = solution
            .assembly
            .transform_to(id, solution.assembly.root())
            .map_err(assembly_error)?;
        metrics.maximum_scale_shift = metrics.maximum_scale_shift.max(map.scale_shift());
        metrics.presented_route_length +=
            measured.route_length as f64 / (1u64 << map.scale_shift()) as f64;
        metrics.frames.insert(id.clone(), measured);
    }
    // Qualified normals make every cross-frame join straight. Internal turns
    // have already been counted in their single owning canonical frame.
    check_cancel(cancelled)?;
    Ok(metrics)
}
