//! Headless comparison against the actual original Architecture renderer.
//! Keep its floating point geometry unchanged; never repair it for the audit.
#[allow(dead_code)]
#[path = "../src/composition.rs"]
mod composition;
#[allow(dead_code)]
#[path = "../src/connections.rs"]
mod connections;
#[path = "support/control_geometry.rs"]
mod control_geometry;
#[allow(dead_code)]
#[path = "../src/primitives.rs"]
mod primitives;

use control_geometry::{audit, capture_layout, capture_legacy, compatible_projection, measure};
use serde_json::json;
use std::{collections::BTreeMap, sync::atomic::AtomicBool, time::Instant};
use visualyze_layout::{Algorithm, LayoutRequest, QualityProfile};
use visualyze_model::architecture::Architecture;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        return Err("supply one or more snapshot JSON paths; bundles and focused projections are unsupported".into());
    }
    let mut records = Vec::new();
    for path in paths {
        let model = Architecture::from_reader(std::fs::File::open(&path)?)?;
        let projection = match compatible_projection(&model) {
            Ok(projection) => projection,
            Err(reason) => {
                records.push(json!({"fixture":path,"status":"unsupported","reason":reason}));
                continue;
            }
        };
        let started = Instant::now();
        let geometry = std::panic::catch_unwind(|| capture_legacy(&model));
        let placement_ms = started.elapsed().as_secs_f64() * 1000.0;
        match geometry {
            Ok(geometry) => {
                let problems = audit(&projection, &geometry);
                records.push(json!({
                    "fixture":path,"algorithm":"LegacyArchitecture",
                    "projection_key":projection.key,"objects":geometry.boxes.len(),
                    "routes":geometry.routes.len(),"status":if problems.is_empty(){"audited"}else{"invalid"},
                    "audit":problems,"metrics":measure(&projection,&geometry),
                    "placement_ms":placement_ms,"exact_validator":"unavailable for unsnapped legacy floating-point geometry"
                }));
            }
            Err(error) => records.push(json!({"fixture":path,"algorithm":"LegacyArchitecture",
                "projection_key":projection.key,"status":"renderer_failed",
                "error":error.downcast_ref::<String>().map(String::as_str)
                    .or_else(||error.downcast_ref::<&str>().copied()).unwrap_or("legacy composition panicked"),
                "placement_ms":placement_ms})),
        }
        for algorithm in [
            Algorithm::Layered,
            Algorithm::Floorplan,
            Algorithm::Constrained,
            Algorithm::Treemap,
        ] {
            let started = Instant::now();
            let result = visualyze_layout::solve(LayoutRequest {
                projection: &projection,
                algorithm,
                pins: &BTreeMap::new(),
                previous: None,
                quality: QualityProfile::default(),
                work_budget: 32,
                cancelled: &AtomicBool::new(false),
            });
            let placement_ms = started.elapsed().as_secs_f64() * 1000.0;
            match result {
                Ok(solution) => {
                    // The shared floating audit is supplemental to the production validator.
                    visualyze_layout::validate(&projection,&solution,&BTreeMap::new(),2)?;
                    let geometry = capture_layout(&projection,&solution);
                    let problems = audit(&projection,&geometry);
                    records.push(json!({"fixture":path,"algorithm":format!("{algorithm:?}"),
                        "projection_key":projection.key,"objects":geometry.boxes.len(),
                        "routes":geometry.routes.len(),"status":if problems.is_empty(){"valid"}else{"audit_disagreement"},
                        "audit":problems,"metrics":measure(&projection,&geometry),
                        "exact_validator":"passed","placement_ms":placement_ms}));
                }
                Err(error) => records.push(json!({"fixture":path,"algorithm":format!("{algorithm:?}"),
                    "projection_key":projection.key,"status":"solve_failed","error":error.to_string(),"placement_ms":placement_ms})),
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "format_version":1,"quality_version":visualyze_layout::QUALITY_VERSION,
            "work_budget":32,"units":"scene units; volume in cubic scene units",
            "scope":"Fully expanded supplied snapshots with identical object/relationship membership and ownership. No bundling, grouping, focused references or synthetic roots. Legacy geometry is copied without resizing, snapping, rerouting or attachment repair. Invalid records remain diagnostic measurements and must not enter valid-winner medians. The shared floating audit supplements, not replaces, the exact solver validator. Projected overlap is a fixed-view rectangular proxy, not a human comprehension metric. No coupled optimality or default-promotion claim.",
            "records":records
        }))?
    );
    Ok(())
}
