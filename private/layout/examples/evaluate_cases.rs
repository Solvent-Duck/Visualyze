//! Machine-readable comparison for a hash-checked development manifest.
//! Run via private/layout/evaluation/run.py; this executable never fetches data.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    sync::atomic::AtomicBool,
    time::Instant,
};
use visualyze_layout::{
    Algorithm, LAYOUT_VERSION, LayoutRequest, LayoutSolution, Projection, QUALITY_VERSION,
    QualityProfile, measure,
    projection::{LayoutPolicy, ViewState, project},
    provider_projection::{ProviderViewRequest, project_provider},
    quality::{NORMALIZATION, SCORING_WEIGHTS, ScoringWeights},
    solve, validate,
};
use visualyze_model::{
    architecture::Architecture,
    provider::{Limits, LocalBundle, ModelProvider, SemanticLens},
};

fn active_profile() -> Value {
    let q = QualityProfile::default();
    let v = ProviderViewRequest::default();
    json!({
        "layout_version": LAYOUT_VERSION, "quality_version": QUALITY_VERSION,
        "normalization": NORMALIZATION, "weights": SCORING_WEIGHTS,
        "solver": {"clearance": q.clearance, "gap": q.gap,
            "routed_candidates": q.routed_candidates, "feedback_candidates": q.feedback_candidates,
            "size_candidates": q.size_candidates, "movement_weight": q.movement_weight},
        "work_budget": 32,
        "projection": {"peer_ceiling": v.peer_ceiling, "group_target": v.group_target,
            "visible_budget": v.visible_budget, "max_children": v.max_children,
            "max_relation_records": v.max_relation_records, "max_references": v.max_references,
            "automatic_groups": v.automatic_groups, "load_focus_detail": v.load_focus_detail},
        "algorithms": ["Layered", "Floorplan", "Constrained", "Treemap"],
        "continuity": "reported separately; no predecessor in initial-placement cases",
        "normalization_equations": {
            "reference_volume": "max(1,sum(preferred_width*preferred_height*preferred_depth for projected solids))",
            "reference_length": "max(1,cbrt(reference_volume))",
            "length": "actual orthogonal routed length / reference_length",
            "compactness": "cbrt(enclosing volume / reference_volume)",
            "aspect": "max enclosing dimension / min enclosing dimension - 1",
            "occlusion": "min(1,pairwise projected solid overlap / total projected solid area) across XY/XZ/YZ",
            "distortion": "sum(L1(actual_size,preferred_size) / reference_length for solids)",
            "congestion": "shared_lane_length/reference_length + crossing_points/max(1,displayed edge count)"
        }
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct View {
    focus: Option<String>,
    expanded: Vec<String>,
    policy: LayoutPolicy,
    lens: Option<SemanticLens>,
    child_page: usize,
    relation_page: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    path: String,
    format: String,
    role: String,
    sha256: String,
    view: View,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    profile: Value,
    cases: Vec<Case>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
struct NormalizationScale {
    reference_volume: f64,
    reference_length: f64,
}

impl NormalizationScale {
    fn from_projection(projection: &Projection) -> Self {
        let reference_volume = projection
            .nodes
            .iter()
            .filter(|node| node.solid)
            .map(|node| {
                node.preferred_size.x as f64
                    * node.preferred_size.y as f64
                    * node.preferred_size.z as f64
            })
            .sum::<f64>()
            .max(1.0);
        Self {
            reference_volume,
            reference_length: reference_volume.cbrt().max(1.0),
        }
    }
}

/// Diagnostic features for weight calibration. Neither these values nor their
/// weighted sum establish human readability or comprehension.
#[derive(Debug, Serialize)]
struct ScoreComponents {
    route_length: f64,
    bends: f64,
    compactness: f64,
    aspect_ratio: f64,
    occlusion: f64,
    distortion: f64,
    congestion: f64,
}

impl ScoreComponents {
    fn weighted_cost(&self, weights: ScoringWeights) -> f64 {
        // Preserve validation::measure's addition order, including congestion
        // being added after the original six terms.
        let structural_cost = self.route_length * weights.route_length
            + self.bends * weights.bends
            + self.compactness * weights.compactness
            + self.aspect_ratio * weights.aspect_ratio
            + self.occlusion * weights.occlusion
            + self.distortion * weights.distortion;
        structural_cost + self.congestion * weights.congestion
    }
}

fn capture_score_components(
    projection: &Projection,
    solution: &LayoutSolution,
    scale: NormalizationScale,
) -> Result<ScoreComponents, String> {
    if scale != NormalizationScale::from_projection(projection) {
        return Err("score normalization differs from the fixed input projection".into());
    }
    // The caller has validated geometry. Re-measure it so calibration features
    // are not accepted merely because a candidate supplied plausible metrics.
    let measured = measure(projection, solution, None);
    let distortion = projection
        .nodes
        .iter()
        .filter(|node| node.solid)
        .map(|node| {
            node.preferred_size
                .manhattan(solution.boxes[&node.id].bounds.size()) as f64
                / scale.reference_length
        })
        .sum();
    let components = ScoreComponents {
        route_length: measured.route_length as f64 / scale.reference_length,
        bends: measured.bends as f64,
        compactness: (measured.volume / scale.reference_volume).cbrt(),
        aspect_ratio: measured.aspect_ratio - 1.0,
        occlusion: measured.occlusion,
        distortion,
        congestion: measured.congestion,
    };
    let reconstructed = components.weighted_cost(SCORING_WEIGHTS);
    let tolerance = 1e-10 * reconstructed.abs().max(1.0);
    for cost in [measured.structural_cost, solution.metrics.structural_cost] {
        if !reconstructed.is_finite()
            || !cost.is_finite()
            || (reconstructed - cost).abs() > tolerance
        {
            return Err("score components do not reproduce the validated structural cost".into());
        }
    }
    Ok(components)
}

fn evaluate(case: &Case) -> Result<Value, Box<dyn std::error::Error>> {
    if case.view.child_page > 64 || case.view.relation_page > 64 {
        return Err("evaluation page traversal exceeds the declared 64-page bound".into());
    }
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let (graph, indexing_ms, projection_ms, completeness) = match case.format.as_str() {
        "snapshot" => {
            if case.view.lens.is_some() || case.view.child_page != 0 || case.view.relation_page != 0
            {
                return Err("snapshot cases cannot silently ignore lens or page settings".into());
            }
            let model = Architecture::from_reader(File::open(&case.path)?)?;
            let indexing_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let mut view = ViewState::default();
            view.focus = case.view.focus.clone();
            view.expanded = case.view.expanded.iter().cloned().collect();
            view.policy = case.view.policy;
            let graph = project(&model, &view)?;
            (
                graph,
                indexing_ms,
                started.elapsed().as_secs_f64() * 1000.0,
                json!({
                    "visible_projection_records_complete": true,
                    "supplied_snapshot_coverage": format!("{:?}", model.snapshot().snapshot.coverage).to_lowercase(),
                    "supplied_omissions": model.snapshot().snapshot.omissions
                }),
            )
        }
        "bundle" => {
            let mut provider = LocalBundle::open(&case.path, Limits::default())?;
            let indexing_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let mut request = ProviderViewRequest {
                focus: case.view.focus.clone(),
                expanded: case.view.expanded.iter().cloned().collect(),
                policy: case.view.policy,
                lens: case.view.lens,
                child_page: case.view.child_page,
                ..ProviderViewRequest::default()
            };
            let mut p = project_provider(&mut provider, &request, &cancel)?;
            for _ in 0..case.view.relation_page {
                request.boundary_after = p.continuations.iter().find_map(|next| match next {
                    visualyze_layout::provider_projection::Continuation::Boundary { after } => {
                        Some(after.clone())
                    }
                    _ => None,
                });
                if request.boundary_after.is_none() {
                    return Err("requested relationship page does not exist".into());
                }
                p = project_provider(&mut provider, &request, &cancel)?;
            }
            if !p.unrepresented_relation_ids.is_empty() {
                return Err(format!(
                    "loaded relationships are unrepresented: {:?}",
                    p.unrepresented_relation_ids
                )
                .into());
            }
            let completeness = json!({"projection_query_complete": p.complete, "remaining_queries": p.continuations.len(),
                "unrepresented_relationships": p.unrepresented_relation_ids, "total_children": p.total_children,
                "supplied_bundle_coverage": provider.manifest().coverage,
                "supplied_omissions": provider.manifest().omissions});
            (
                p.graph,
                indexing_ms,
                started.elapsed().as_secs_f64() * 1000.0,
                completeness,
            )
        }
        _ => return Err("unknown fixture format".into()),
    };
    for requested in &case.view.expanded {
        let children: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| node.parent.as_ref() == Some(requested))
            .collect();
        // Exposed interfaces remain attached pads on a solid owner. Ordinary
        // interior children require an open frame; attached pads do not.
        let applied = graph
            .nodes
            .iter()
            .find(|node| &node.id == requested)
            .is_some_and(|owner| {
                !children.is_empty()
                    && (!owner.solid
                        || children
                            .iter()
                            .all(|child| child.kind == visualyze_layout::NodeKind::Interface))
            });
        if !applied {
            return Err(format!(
                "requested expansion was not represented with children: {requested}"
            )
            .into());
        }
    }
    let normalization_scale = NormalizationScale::from_projection(&graph);
    let pins = BTreeMap::new();
    let mut rows = vec![];
    for algorithm in [
        Algorithm::Layered,
        Algorithm::Floorplan,
        Algorithm::Constrained,
        Algorithm::Treemap,
    ] {
        let started = Instant::now();
        let result = solve(LayoutRequest {
            projection: &graph,
            algorithm,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &cancel,
        })
        .map_err(|error| error.to_string())
        .and_then(|solution| {
            validate(
                &graph,
                &solution,
                &pins,
                QualityProfile::default().clearance,
            )
            .map_err(|error| error.to_string())?;
            let components = capture_score_components(&graph, &solution, normalization_scale)?;
            Ok((solution, components))
        });
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        rows.push(match result {
            Ok((solution, components)) => json!({"algorithm": format!("{algorithm:?}"), "status": "valid",
                "milliseconds": elapsed_ms, "metrics": solution.metrics,
                "score_components": components,
                "objects": solution.boxes.len(), "routes": solution.routes.len(),
                "solution": solution}),
            Err(error) => json!({"algorithm": format!("{algorithm:?}"), "status": "failed",
                "milliseconds": elapsed_ms, "error": error.to_string()}),
        });
    }
    Ok(
        json!({"case": case, "model_id": graph.model_id, "revision": graph.revision,
        "projection_key": graph.key, "completeness": completeness,
        "normalization_scale": normalization_scale,
        "indexing_ms": indexing_ms, "projection_ms": projection_ms, "results": rows}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--profile"] {
        println!("{}", serde_json::to_string_pretty(&active_profile())?);
        return Ok(());
    }
    if args.len() != 2 {
        return Err("usage: evaluate_cases --profile | REQUEST_JSON NEW_REPORT_JSON".into());
    }
    let request: Request = serde_json::from_reader(File::open(&args[0])?)?;
    if request.profile != active_profile() {
        return Err("frozen profile differs from this executable; comparison refused".into());
    }
    if request.cases.is_empty() || request.cases.len() > 128 {
        return Err("expected 1..128 declared evaluation cases".into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    let mut reports = vec![];
    for case in request.cases {
        reports.push(match evaluate(&case) {
            Ok(report) => report,
            Err(error) => {
                json!({"case": case, "status": "projection_failed", "error": error.to_string()})
            }
        });
    }
    serde_json::to_writer_pretty(
        &mut output,
        &json!({"profile": active_profile(), "cases": reports}),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unrepresented_snapshot_page_or_lens_settings() {
        let mut case = Case {
            id: "bad-page".into(),
            path: "unused".into(),
            format: "snapshot".into(),
            role: "development".into(),
            sha256: "unused".into(),
            view: View {
                focus: None,
                expanded: Vec::new(),
                policy: LayoutPolicy::Crud,
                lens: None,
                child_page: 1,
                relation_page: 0,
            },
        };
        assert!(
            evaluate(&case)
                .unwrap_err()
                .to_string()
                .contains("silently ignore")
        );
        case.view.child_page = 0;
        case.view.lens = Some(SemanticLens::Code);
        assert!(
            evaluate(&case)
                .unwrap_err()
                .to_string()
                .contains("silently ignore")
        );
        case.view.relation_page = 65;
        assert!(evaluate(&case).unwrap_err().to_string().contains("64-page"));
    }

    #[test]
    fn interface_expansion_preserves_solid_owner_and_unknown_expansion_fails() {
        let mut case = Case {
            id: "interfaces".into(),
            path: concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../public/examples/notes-crud.json"
            )
            .into(),
            format: "snapshot".into(),
            role: "training".into(),
            sha256: "test".into(),
            view: View {
                focus: None,
                expanded: vec!["service:notes".into()],
                policy: LayoutPolicy::Crud,
                lens: None,
                child_page: 0,
                relation_page: 0,
            },
        };
        let report = evaluate(&case).unwrap();
        for result in report["results"].as_array().unwrap() {
            assert_eq!(result["status"], "valid");
            assert_eq!(result["solution"]["boxes"]["service:notes"]["solid"], true);
        }
        case.view.expanded = vec!["service:not-supplied".into()];
        assert!(
            evaluate(&case)
                .unwrap_err()
                .to_string()
                .contains("not represented")
        );
    }
}
