use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    projection::{ViewState, project},
    *,
};
use visualyze_model::architecture::Architecture;
fn model() -> Architecture {
    Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json")).unwrap()
}
fn run(p: &Projection, a: Algorithm) -> LayoutSolution {
    solve(LayoutRequest {
        projection: p,
        algorithm: a,
        pins: &BTreeMap::new(),
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 16,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap()
}
#[test]
fn every_candidate_routes_every_projected_claim_and_validates() {
    for bytes in [
        include_bytes!("../../../public/examples/notes-crud.json").as_slice(),
        include_bytes!("../../../public/examples/sketch-crud.json").as_slice(),
    ] {
        let model = Architecture::from_json(bytes).unwrap();
        let p = project(&model, &ViewState::default()).unwrap();
        for a in [
            Algorithm::Layered,
            Algorithm::Floorplan,
            Algorithm::Constrained,
            Algorithm::Treemap,
        ] {
            let s = run(&p, a);
            validate(&p, &s, &BTreeMap::new(), 2).unwrap();
            assert_eq!(s.routes.len(), p.edges.len());
            assert!(s.metrics.structural_cost.is_finite());
        }
    }
}
#[test]
fn deterministic_under_entity_relation_and_evidence_reordering() {
    let model = model();
    let p = project(&model, &ViewState::default()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json")).unwrap();
    for field in ["entities", "relations", "evidence"] {
        value[field].as_array_mut().unwrap().reverse();
    }
    let other = Architecture::from_json(value.to_string().as_bytes()).unwrap();
    let q = project(&other, &ViewState::default()).unwrap();
    assert_eq!(run(&p, Algorithm::Floorplan), run(&q, Algorithm::Floorplan));
}
#[test]
fn cancelled_conflicting_pins_and_tampered_solution_are_rejected() {
    let p = project(&model(), &ViewState::default()).unwrap();
    let pins = BTreeMap::new();
    let cancelled = AtomicBool::new(true);
    let active = AtomicBool::new(false);
    let req = |pins, cancelled| LayoutRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 4,
        cancelled,
    };
    assert_eq!(solve(req(&pins, &cancelled)), Err(LayoutError::Cancelled));
    let pins = BTreeMap::from([
        ("service:notes".into(), Point::ZERO),
        ("store:notes".into(), Point::ZERO),
    ]);
    assert!(solve(req(&pins, &active)).is_err());
    let mut s = run(&p, Algorithm::Floorplan);
    s.revision += 1;
    assert!(validate(&p, &s, &BTreeMap::new(), 2).is_err());
}
#[test]
fn unchanged_update_preserves_every_position() {
    let p = project(&model(), &ViewState::default()).unwrap();
    let a = run(&p, Algorithm::Floorplan);
    let b = solve(LayoutRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        pins: &BTreeMap::new(),
        previous: Some(&a),
        quality: QualityProfile::default(),
        work_budget: 16,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    assert_eq!(a.boxes, b.boxes);
    assert_eq!(b.metrics.displacement, 0);
}
