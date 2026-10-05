//! Streaming synthetic hierarchy benchmark; does not instantiate a scene.
use std::{error::Error, time::Instant};

use visualyze_model::provider::*;

fn id(i: usize) -> String {
    format!("entity:{i:08}")
}

fn main() -> Result<(), Box<dyn Error>> {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "10000".into())
        .parse()?;
    if !(2..=2_000_000).contains(&count) {
        return Err("benchmark count must be in 2..=2000000".into());
    }
    let path = std::env::temp_dir().join(format!(
        "visualyze-bundle-bench-{}-{count}.jsonl",
        std::process::id()
    ));
    let manifest = Manifest {
        format: FORMAT_VERSION.into(),
        model_id: "synthetic:hierarchy".into(),
        revision: 1,
        root_id: id(0),
        profiles: vec![Profile::Architecture],
        coverage: Coverage::Complete,
        scope_description:
            "Synthetic 32-way ownership tree with one contains relation per non-root entity".into(),
        omissions: vec![
            "No claim about any real system; no viewer layout or rendering in this benchmark"
                .into(),
        ],
    };
    let start = Instant::now();
    let mut writer = BundleWriter::create(&path, manifest, Limits::default())?;
    for i in 0..count {
        writer.push(&BundleRecord::Entity(EntityRecord {
            summary: EntitySummary {
                id: id(i),
                kind: if i * 32 + 1 < count {
                    EntityKind::Scope
                } else {
                    EntityKind::Service
                },
                label: format!("Synthetic object {i}"),
                boundary: Boundary::Internal,
                claim: Claim::declared(),
            },
            detail: Some(EntityDetail {
                description: "Synthetic bounded descriptive detail for measuring demand loading."
                    .into(),
                subtype: None,
                roles: vec![],
                technology: vec![],
            }),
        }))?;
    }
    for i in 1..count {
        writer.push(&BundleRecord::Relation(RelationRecord {
            id: format!("relation:{i:08}"),
            kind: RelationKind::Contains,
            source: id((i - 1) / 32),
            target: id(i),
            claim: Claim::declared(),
            label: None,
            aspect: None,
        }))?;
    }
    let write_seconds = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let mut provider = writer.finish()?;
    let indexing_seconds = start.elapsed().as_secs_f64();
    let cold_stats = provider.stats();
    let start = Instant::now();
    let children = provider.children(1, &id(0), PageRequest::default())?;
    let root_page_micros = start.elapsed().as_micros();
    let start = Instant::now();
    for i in 0..100.min(count) {
        provider.detail(1, &id((i * 7919) % count))?;
    }
    let cold_details_micros = start.elapsed().as_micros();
    let start = Instant::now();
    for i in 0..100.min(count) {
        provider.detail(1, &id((i * 7919) % count))?;
    }
    let cached_details_micros = start.elapsed().as_micros();
    let stats = provider.stats();
    println!(
        "{}",
        serde_json::json!({
            "entities": count,
            "relations": stats.counts.relations,
            "bundle_bytes": std::fs::metadata(&path)?.len(),
            "stream_write_seconds": write_seconds,
            "index_validation_seconds": indexing_seconds,
            "estimated_index_bytes": stats.estimated_index_bytes,
            "cached_details_before_queries": cold_stats.cached_records,
            "root_page_count": children.items.len(),
            "root_page_micros": root_page_micros,
            "one_hundred_cold_details_micros": cold_details_micros,
            "one_hundred_cached_details_micros": cached_details_micros,
            "cached_detail_records": stats.cached_records,
            "cached_detail_bytes": stats.cached_bytes,
        })
    );
    drop(provider);
    std::fs::remove_file(path)?;
    Ok(())
}
