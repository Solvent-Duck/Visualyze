//! Reproducible headless scale proof: write/index -> bounded projection -> routing.
//! Usage: scale_view_bench COUNT REPORT_JSON [NEW_BUNDLE_PATH]
//! GPU frames and native cache transitions are measured by production_smoke.
use std::{
    collections::BTreeMap, error::Error, path::PathBuf, sync::atomic::AtomicBool, time::Instant,
};

use visualyze_layout::{
    Algorithm, LayoutRequest, QualityProfile,
    projection::LayoutPolicy,
    provider_projection::{ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::provider::*;

const FANOUT: usize = 32;
fn id(i: usize) -> String {
    format!("entity:{i:08}")
}

fn relation(name: &str, kind: RelationKind, source: usize, target: usize) -> BundleRecord {
    BundleRecord::Relation(RelationRecord {
        id: format!("relation:{name}:{source:08}:{target:08}"),
        kind,
        source: id(source),
        target: id(target),
        claim: Claim::declared(),
        label: None,
        aspect: None,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let count: usize = args.next().ok_or("expected entity count")?.parse()?;
    let output = PathBuf::from(args.next().ok_or("expected new report path")?);
    let keep_bundle = args.next().map(PathBuf::from);
    if args.next().is_some() || !(1_024..=1_000_000).contains(&count) {
        return Err("usage: scale_view_bench COUNT(1024..1000000) REPORT [NEW_BUNDLE]".into());
    }
    // Refuse to overwrite evidence from an earlier measurement.
    let report_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let path = keep_bundle.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "visualyze-scale-view-{}-{count}.jsonl",
            std::process::id()
        ))
    });
    let limits = Limits::default();
    let started = Instant::now();
    let first_leaf = (count - 2) / FANOUT + 1;
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: format!("synthetic:scale-view:{count}"),
            revision: 1,
            root_id: id(0),
            profiles: vec![Profile::Architecture],
            coverage: Coverage::Complete,
            scope_description: "Declared 32-way ownership tree. Every eighth leaf depends on its next peer; every sixty-fourth leaf also depends on one shared leaf. All dependencies have unknown runtime behavior.".into(),
            omissions: vec!["Synthetic scale workload, not a real company's architecture. No deployment, code, traffic, execution or capacity claims.".into()],
        },
        limits.clone(),
    )?;
    for i in 0..count {
        writer.push(&BundleRecord::Entity(EntityRecord {
            summary: EntitySummary {
                id: id(i),
                kind: if i < first_leaf {
                    EntityKind::Scope
                } else {
                    EntityKind::Service
                },
                label: format!("Synthetic object {i}"),
                boundary: Boundary::Internal,
                claim: Claim::declared(),
            },
            detail: Some(EntityDetail {
                description: "Declared synthetic region or service with bounded supplied detail."
                    .into(),
                subtype: None,
                roles: vec![],
                technology: vec![],
            }),
        }))?;
    }
    for i in 1..count {
        writer.push(&relation(
            "owns",
            RelationKind::Contains,
            (i - 1) / FANOUT,
            i,
        ))?;
    }
    let mut dependencies = 0;
    for i in (first_leaf..count).step_by(8) {
        let next = if i + 1 < count { i + 1 } else { first_leaf };
        if next != i {
            writer.push(&relation("peer", RelationKind::DependsOn, i, next))?;
            dependencies += 1;
        }
    }
    for i in (first_leaf + 64..count).step_by(64) {
        writer.push(&relation("shared", RelationKind::DependsOn, i, first_leaf))?;
        dependencies += 1;
    }
    let write_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let mut provider = writer.finish()?;
    let index_ms = started.elapsed().as_secs_f64() * 1000.0;
    let stats_after_index = provider.stats();
    assert_eq!(stats_after_index.counts.entities, count);
    assert_eq!(stats_after_index.counts.relations, count - 1 + dependencies);
    assert_eq!(stats_after_index.cached_records, 0);
    let cancel = AtomicBool::new(false);
    let pins = BTreeMap::new();
    let quality = QualityProfile::default();
    // Different depths and distant leaf regions. A final repeated root is a
    // fresh layout (provider index warm), not a pretend native cache hit.
    let mut focuses = vec![0, 1, 2, (first_leaf - 1) / 2, first_leaf - 1];
    focuses.sort_unstable();
    focuses.dedup();
    focuses.push(0);
    let mut regions = vec![];
    let mut all_original_routes = 0;
    for focus in focuses {
        let start = Instant::now();
        let request = ProviderViewRequest {
            focus: Some(id(focus)),
            policy: LayoutPolicy::Peers,
            ..Default::default()
        };
        let projection = project_provider(&mut provider, &request, &cancel)?;
        let projection_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(
            projection.unrepresented_relation_ids.is_empty(),
            "loaded claims must have representations"
        );
        let start = Instant::now();
        let solution = solve(LayoutRequest {
            projection: &projection.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality,
            work_budget: 48,
            cancelled: &cancel,
        })?;
        let solve_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        validate(&projection.graph, &solution, &pins, quality.clearance)?;
        let validation_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(solution.boxes.len() <= request.visible_budget);
        assert_eq!(solution.routes.len(), projection.graph.edges.len());
        for route in &solution.routes {
            assert!(
                route
                    .semantic_ids
                    .iter()
                    .all(|id| projection.relations.contains_key(id))
            );
        }
        for object in solution.boxes.values() {
            for semantic_id in &object.semantic_ids {
                assert!(provider.summary(1, semantic_id)?.is_some());
            }
        }
        let original_routes = solution
            .routes
            .iter()
            .map(|r| r.semantic_ids.len())
            .sum::<usize>();
        all_original_routes += original_routes;
        // Measure reusing a validated accepted arrangement with the public repair
        // API. Native cache/rebuild costs intentionally remain separate.
        let start = Instant::now();
        let unchanged = solve(LayoutRequest {
            projection: &projection.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: Some(&solution),
            quality,
            work_budget: 48,
            cancelled: &cancel,
        })?;
        let reuse_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            solution.boxes, unchanged.boxes,
            "unchanged accepted layout moved"
        );
        regions.push(serde_json::json!({
            "focus":id(focus), "objects":solution.boxes.len(), "routes":solution.routes.len(),
            "original_routed_relations":original_routes,
            "loaded_claims":projection.relations.len(),
            "internal_claims_sample":projection.graph.internal_relation_ids.len(),
            "remaining_query_pages":projection.continuations.len(),
            "projection_complete":projection.complete,
            "projection_ms":projection_ms, "solve_ms":solve_ms,
            "independent_validation_ms":validation_ms, "accepted_layout_reuse_ms":reuse_ms,
            "routed_length":solution.metrics.route_length, "bends":solution.metrics.bends,
            "volume":solution.metrics.volume,
            "shared_lane_length":solution.metrics.shared_lane_length,
            "crossing_points":solution.metrics.crossing_points,
            "congestion":solution.metrics.congestion,
        }));
    }
    assert!(
        all_original_routes > 0,
        "the scale proof must actually route dependencies"
    );
    // Exhaust the real provider LRU, checking that detail returns from disk with
    // the same supplied content. No artificial eviction or substitute cache.
    provider.detail(1, &id(0))?;
    let original_detail = provider.detail(1, &id(0))?.detail.unwrap();
    for i in 1..=limits.cache_records + 1 {
        provider.detail(1, &id(i))?;
    }
    assert_eq!(provider.detail_state(1, &id(0))?, DetailState::Unloaded);
    assert!(provider.stats().cached_records <= limits.cache_records);
    assert!(provider.stats().cached_bytes <= limits.cache_bytes);
    let start = Instant::now();
    assert_eq!(
        provider.detail(1, &id(0))?.detail.as_deref(),
        Some(original_detail.as_ref())
    );
    let evicted_detail_reload_ms = start.elapsed().as_secs_f64() * 1000.0;
    let stats = provider.stats();
    serde_json::to_writer_pretty(
        report_file,
        &serde_json::json!({
            "workload":"32-way declared hierarchy with sparse peer and shared dependencies",
        "scope":"Headless provider, six sampled bounded projections, actual routed placement, independent validation and accepted-layout reuse. Not an exhaustive traversal of all regions or relationship pages. Does not measure GPU, native scene rebuild, frame rate, or native layout-cache navigation. Timings use recently-written files (warm OS cache).",
        "layout_version":visualyze_layout::LAYOUT_VERSION,
        "quality_version":visualyze_layout::QUALITY_VERSION,
        "feedback_candidate_budget":quality.feedback_candidates,
        "quality_profile":{"clearance":quality.clearance,"gap":quality.gap,
            "routed_candidates":quality.routed_candidates,"feedback_candidates":quality.feedback_candidates,
            "movement_weight":quality.movement_weight},
            "work_budget":48, "entities":count, "relations":stats.counts.relations,
            "non_ownership_dependencies":dependencies, "bundle_bytes":std::fs::metadata(&path)?.len(),
        "stream_write_ms":write_ms, "finalize_and_index_validation_ms":index_ms,
            "estimated_index_bytes":stats.estimated_index_bytes,
            "cached_details_after_index":stats_after_index.cached_records,
            "cached_details_final":stats.cached_records, "cached_detail_bytes_final":stats.cached_bytes,
            "evicted_detail_reload_ms":evicted_detail_reload_ms,
            "regions":regions,
        }),
    )?;
    drop(provider);
    if keep_bundle.is_none() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
