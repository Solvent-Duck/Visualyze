//! Inspect and solve one demand-loaded bundle region without a GPU.
//! Usage: cargo run -p visualyze-layout --example bundle_layout -- BUNDLE [FOCUS_ID]
use std::{collections::BTreeMap, sync::atomic::AtomicBool, time::Instant};
use visualyze_layout::{
    Algorithm, LayoutRequest, QualityProfile,
    provider_projection::{ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::provider::{Limits, LocalBundle, ModelProvider};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: bundle_layout BUNDLE [FOCUS_ID]")?;
    let focus = args.next();
    if args.next().is_some() {
        return Err("usage: bundle_layout BUNDLE [FOCUS_ID]".into());
    }
    let started = Instant::now();
    let mut provider = LocalBundle::open(&path, Limits::default())?;
    let indexing_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let started = Instant::now();
    let projection = project_provider(
        &mut provider,
        &ProviderViewRequest {
            focus,
            ..ProviderViewRequest::default()
        },
        &AtomicBool::new(false),
    )?;
    let projection_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let started = Instant::now();
    let pins = BTreeMap::new();
    let solution = solve(LayoutRequest {
        projection: &projection.graph,
        algorithm: Algorithm::Floorplan,
        pins: &pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 32,
        cancelled: &AtomicBool::new(false),
    })?;
    validate(&projection.graph, &solution, &pins, 2)?;
    let layout_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let stats = provider.stats();
    let report = serde_json::json!({
        "model_id": provider.manifest().model_id,
        "revision": provider.manifest().revision,
        "focus": projection.graph.root,
        "layout_version": solution.version,
        "geometry": "valid",
        "local_projection_complete": projection.complete,
        "hidden_descendant_edges_aggregated": true,
        "aggregation_scope": "Current bounded descendant relationship scan; remaining queries and completeness are reported separately.",
        "remaining_queries": projection.continuations.len(),
        "unrepresented_loaded_relationships": projection.unrepresented_relation_ids,
        "diagnostics": projection.graph.diagnostics,
        "source_counts": { "entities": stats.counts.entities, "relations": stats.counts.relations },
        "resident": { "estimated_index_bytes": stats.estimated_index_bytes,
            "cached_detail_records": stats.cached_records, "cached_detail_bytes": stats.cached_bytes },
        "visible": { "objects": solution.boxes.len(), "relationships": solution.routes.len(),
            "references": projection.graph.references.len() },
        "timing_ms": { "indexing": indexing_ms, "projection": projection_ms, "layout": layout_ms },
        "quality_version": visualyze_layout::QUALITY_VERSION,
        "quality": { "routed_length": solution.metrics.route_length, "bends": solution.metrics.bends,
            "volume": solution.metrics.volume, "aspect_ratio": solution.metrics.aspect_ratio,
            "occlusion_proxy": solution.metrics.occlusion,
            "shared_lane_length": solution.metrics.shared_lane_length,
            "crossing_points": solution.metrics.crossing_points,
            "congestion": solution.metrics.congestion,
            "structural_cost": solution.metrics.structural_cost },
        "semantic_entities": projection.entities.values().collect::<Vec<_>>(),
        "semantic_relations": projection.relations.values().collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
