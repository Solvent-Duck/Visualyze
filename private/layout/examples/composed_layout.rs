//! Solve a supplied bundle region with independent canonical interiors.
//! Usage: composed_layout BUNDLE FOCUS_OR_DASH crud|code [--feedback 0..16] [EXPANDED_ID ...]
//! This is a headless development probe, not the native viewer/default engine.
use std::{collections::BTreeSet, sync::atomic::AtomicBool, time::Instant};
use visualyze_layout::{
    Algorithm, QualityProfile,
    composed::{CanonicalPins, ComposedRequest, solve_composed, validate_composed},
    projection::LayoutPolicy,
    provider_projection::{ProviderViewRequest, project_provider},
};
use visualyze_model::provider::{Limits, LocalBundle, ModelProvider};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: composed_layout BUNDLE FOCUS_OR_DASH crud|code [--feedback 0..16] [EXPANDED_ID ...]")?;
    let focus = args.next().ok_or("missing focus ID (use - for overview)")?;
    let policy = match args.next().as_deref() {
        Some("crud") => LayoutPolicy::Crud,
        Some("code") => LayoutPolicy::Code,
        _ => return Err("policy must be crud or code".into()),
    };
    let mut expanded = BTreeSet::new();
    let mut feedback = 4;
    while let Some(arg) = args.next() {
        if arg == "--feedback" {
            feedback = args
                .next()
                .ok_or("missing feedback candidate count")?
                .parse::<usize>()?;
            if feedback > 16 {
                return Err("feedback count must be 0..16".into());
            }
        } else {
            expanded.insert(arg);
        }
    }
    let started = Instant::now();
    let mut provider = LocalBundle::open(&path, Limits::default())?;
    let indexing_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let started = Instant::now();
    let cancelled = AtomicBool::new(false);
    let projection = project_provider(
        &mut provider,
        &ProviderViewRequest {
            focus: (focus != "-").then_some(focus),
            expanded: expanded.clone(),
            policy,
            ..Default::default()
        },
        &cancelled,
    )?;
    let projection_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let pins = CanonicalPins::new();
    let started = Instant::now();
    let result = solve_composed(ComposedRequest {
        projection: &projection.graph,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile {
            feedback_candidates: feedback,
            ..Default::default()
        },
        pins: &pins,
        work_budget: 192,
        cancelled: &cancelled,
    });
    let layout_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let stats = provider.stats();
    let mut report = serde_json::json!({
        "probe": "headless canonical composition; no native rendering claim",
        "feedback_candidates": feedback,
        "feedback_control_scope": "Profile feedback allowance applies to flat kernels and composed boundary refinement. Zero disables both; changing this setting also changes shared work allocation. This is not a pure isolated-stage timing comparison.",
        "model_id": projection.graph.model_id, "revision": projection.graph.revision,
        "focus": projection.graph.root, "expanded": expanded,
        "source_counts": {"entities": stats.counts.entities, "relationships": stats.counts.relations},
        "projection": {"objects": projection.graph.nodes.len(), "relationships": projection.graph.edges.len(),
            "complete": projection.complete, "remaining_queries": projection.continuations.len(),
            "unrepresented_loaded_relationships": projection.unrepresented_relation_ids,
            "diagnostics": projection.graph.diagnostics},
        "timing_ms": {"indexing": indexing_ms, "projection": projection_ms, "composition": layout_ms},
        "quality_version": visualyze_layout::QUALITY_VERSION,
        "composition_version": visualyze_layout::composed::COMPOSITION_VERSION,
        "resident": {"estimated_index_bytes": stats.estimated_index_bytes, "cached_detail_records": stats.cached_records,
            "cached_detail_bytes": stats.cached_bytes},
    });
    match result {
        Ok(solution) => {
            validate_composed(&projection.graph, &solution, &pins, &cancelled)?;
            let presentation = solution.present(&cancelled)?;
            for object in presentation.boxes.values() {
                for id in &object.semantic_ids {
                    if provider.summary(projection.graph.revision, id)?.is_none() {
                        return Err(format!("missing original object {id}").into());
                    }
                }
            }
            for route in &presentation.routes {
                if route
                    .semantic_ids
                    .iter()
                    .any(|id| !projection.relations.contains_key(id))
                {
                    return Err(
                        format!("missing original relationship in {}", route.edge_id).into(),
                    );
                }
            }
            report["status"] = "valid".into();
            report["frames"] = serde_json::json!(solution.assembly.frame_ids().collect::<Vec<_>>());
            report["presented"] = serde_json::json!({"objects": presentation.boxes.len(), "relationships": presentation.routes.len(),
                "fragments": solution.routes.values().map(|r| r.fragments.len()).sum::<usize>()});
            report["work"] = serde_json::json!({"budget": solution.work.budget, "allocations": solution.work.allocated,
                "evaluations": solution.work.evaluations, "frame_evaluations": solution.work.evaluations.values().sum::<usize>(),
                "feedback_evaluations": solution.work.feedback_evaluations,
                "feedback_improvements": solution.work.feedback_improvements,
                "total_evaluations": solution.work.evaluations.values().sum::<usize>() + solution.work.feedback_evaluations});
            report["quality"] = serde_json::json!({"canonical_route_length": solution.metrics.canonical_route_length,
                "presented_route_length": solution.metrics.presented_route_length, "bends": solution.metrics.bends,
                "structural_cost": solution.metrics.structural_cost, "maximum_scale_shift": solution.metrics.maximum_scale_shift,
                "canonical_frames": solution.metrics.frames});
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Err(error) => {
            report["status"] = "failed".into();
            report["error"] = error.to_string().into();
            println!("{}", serde_json::to_string_pretty(&report)?);
            Err(error.into())
        }
    }
}
