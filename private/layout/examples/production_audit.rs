//! Validate real supplied code at repository, package, and file inspection scales.
//! Usage: production_audit BUNDLE OUTPUT_JSON [additional semantic focus IDs...]
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
    time::Instant,
};
use visualyze_layout::{
    Algorithm, LayoutRequest, QualityProfile,
    noise::visual_noise,
    projection::LayoutPolicy,
    provider_projection::{ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::provider::{
    EntityKind, EntitySummary, Limits, LocalBundle, ModelProvider, PageRequest,
};

fn children(
    p: &mut LocalBundle,
    revision: u64,
    id: &str,
) -> Result<Vec<EntitySummary>, Box<dyn std::error::Error>> {
    let mut result = vec![];
    let mut after = None;
    loop {
        let page = p.children(revision, id, PageRequest { limit: 128, after })?;
        result.extend(page.items);
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    Ok(result)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: production_audit BUNDLE OUTPUT_JSON [FOCUS_IDS...]")?;
    let output = args.next().ok_or("missing output report path")?;
    let started = Instant::now();
    let mut provider = LocalBundle::open(&path, Limits::default())?;
    let index_ms = started.elapsed().as_secs_f64() * 1000.0;
    let manifest = provider.manifest().clone();
    let initial_stats = provider.stats();
    let mut focuses = BTreeSet::from([manifest.root_id.clone()]);
    focuses.extend(args);
    let mut packages = children(&mut provider, manifest.revision, &manifest.root_id)?;
    for repository in packages
        .clone()
        .iter()
        .filter(|e| e.kind == EntityKind::CodeRepository)
    {
        focuses.insert(repository.id.clone());
        packages.extend(children(&mut provider, manifest.revision, &repository.id)?);
    }
    for package in packages
        .iter()
        .filter(|e| e.kind == EntityKind::CodePackage)
    {
        focuses.insert(package.id.clone());
        let files = children(&mut provider, manifest.revision, &package.id)?;
        // One deterministic file per package supplements every package view.
        if let Some(file) = files.iter().find(|f| f.kind == EntityKind::CodeFile) {
            focuses.insert(file.id.clone());
        }
    }
    let mut reports = vec![];
    let mut failed = 0;
    for focus in focuses {
        let start = Instant::now();
        let p = project_provider(
            &mut provider,
            &ProviderViewRequest {
                focus: Some(focus.clone()),
                policy: LayoutPolicy::Code,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )?;
        let projection_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(
            p.unrepresented_relation_ids.is_empty(),
            "{focus}: every loaded claim must have a displayed endpoint or internal membership"
        );
        let start = Instant::now();
        let pins = BTreeMap::new();
        let result = solve(LayoutRequest {
            projection: &p.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &AtomicBool::new(false),
        });
        let layout_ms = start.elapsed().as_secs_f64() * 1000.0;
        let label = p.entities.get(&focus).map(|e| e.label.clone());
        let mut report = serde_json::json!({"focus":focus, "label":label,
            "projection_ms":projection_ms, "layout_ms":layout_ms,
            "local_projection_complete":p.complete, "pending_queries":p.continuations.len(),
            "original_relations_loaded":p.relations.len(),
            "internal_original_relations":p.graph.internal_relation_ids.len(),
            "unrepresented_loaded_relations":p.unrepresented_relation_ids.len(),
            "visual_noise":visual_noise(&p.graph),
            "diagnostics":p.graph.diagnostics});
        match result {
            Ok(s) => {
                validate(&p.graph, &s, &pins, 2)?;
                for route in &s.routes {
                    assert!(
                        route
                            .semantic_ids
                            .iter()
                            .all(|id| p.relations.contains_key(id)),
                        "displayed relation must resolve to an original supplied record"
                    );
                }
                for object in s.boxes.values() {
                    for id in &object.semantic_ids {
                        assert!(
                            provider.summary(manifest.revision, id)?.is_some(),
                            "displayed object must resolve to a supplied semantic record"
                        );
                    }
                }
                assert!(s.boxes.len() <= 250);
                report["status"] = "valid".into();
                report["objects"] = s.boxes.len().into();
                report["routes"] = s.routes.len().into();
                report["original_routed_relations"] = s
                    .routes
                    .iter()
                    .map(|r| r.semantic_ids.len())
                    .sum::<usize>()
                    .into();
                report["scope_ports"] = s
                    .routes
                    .iter()
                    .map(|r| r.scope_ports.len())
                    .sum::<usize>()
                    .into();
                report["routed_length"] = s.metrics.route_length.into();
                report["bends"] = s.metrics.bends.into();
                report["volume"] = s.metrics.volume.into();
                report["shared_lane_length"] = s.metrics.shared_lane_length.into();
                report["crossing_points"] = s.metrics.crossing_points.into();
                report["congestion"] = s.metrics.congestion.into();
            }
            Err(error) => {
                failed += 1;
                report["status"] = "failed".into();
                report["error"] = error.to_string().into();
            }
        }
        eprintln!(
            "{}: {} ({layout_ms:.1} ms)",
            report["label"], report["status"]
        );
        reports.push(report);
    }
    let report = serde_json::json!({"model_id":manifest.model_id,"revision":manifest.revision,
        "layout_version":visualyze_layout::LAYOUT_VERSION,
        "quality_version":visualyze_layout::QUALITY_VERSION,
        "source_counts":initial_stats.counts,"index_ms":index_ms,
        "estimated_index_bytes":initial_stats.estimated_index_bytes,
        "inspected_regions":reports.len(),"failed_regions":failed,
        "cached_records":provider.stats().cached_records,"cached_bytes":provider.stats().cached_bytes,
        "scope":"Every root package plus one deterministic source file per package and explicit focus IDs. Bounded first pages; paging is tested in the native proof.",
        "regions":reports});
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    serde_json::to_writer_pretty(file, &report)?;
    println!("Report: {output}");
    if failed > 0 {
        return Err(format!("{failed} production regions could not be laid out").into());
    }
    Ok(())
}
