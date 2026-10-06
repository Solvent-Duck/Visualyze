use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, LayoutRequest, Node, NodeKind, Point, Projection, QualityProfile,
    solve, solve_budgeted, validate,
};

fn node(id: &str, parent: Option<&str>, size: Point) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        kind: if parent.is_some() {
            NodeKind::Service
        } else {
            NodeKind::Scope
        },
        band: 0,
        min_size: size,
        max_size: size,
        preferred_size: size,
        solid: parent.is_some(),
    }
}

fn fixture() -> Projection {
    Projection {
        key: "strict-budget".into(),
        model_id: "strict-budget".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, Point::new(100, 100, 100)),
            node("a", Some("root"), Point::new(40, 40, 40)),
            node("b", Some("root"), Point::new(40, 40, 40)),
        ],
        edges: vec![Edge {
            bundle: None,
            id: "call".into(),
            source: "a".into(),
            target: "b".into(),
            semantic_ids: vec!["call".into()],
        }],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    }
}

fn request<'a>(
    projection: &'a Projection,
    pins: &'a BTreeMap<String, Point>,
    cancelled: &'a AtomicBool,
    work_budget: usize,
) -> LayoutRequest<'a> {
    LayoutRequest {
        projection,
        algorithm: Algorithm::Floorplan,
        pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget,
        cancelled,
    }
}

#[test]
fn zero_allowance_rejects_and_one_allowance_evaluates_only_one_candidate() {
    let projection = fixture();
    let pins = BTreeMap::new();
    let cancelled = AtomicBool::new(false);
    assert!(matches!(
        solve_budgeted(request(&projection, &pins, &cancelled, 0)),
        Err(LayoutError::Invalid(message)) if message.contains("empty budget")
    ));
    let result = solve_budgeted(request(&projection, &pins, &cancelled, 1)).unwrap();
    assert_eq!(result.evaluations, 1);
    validate(&projection, &result.solution, &pins, 2).unwrap();
    assert_eq!(result.solution.routes.len(), 1);
    assert_eq!(
        solve_budgeted(request(&projection, &pins, &AtomicBool::new(true), 1)),
        Err(LayoutError::Cancelled)
    );
}

#[test]
fn reused_accepted_geometry_spends_one_evaluation_even_with_large_allowance() {
    let projection = fixture();
    let pins = BTreeMap::new();
    let cancelled = AtomicBool::new(false);
    let previous = solve(request(&projection, &pins, &cancelled, 8)).unwrap();
    for allowance in [1, 8, 512] {
        let result = solve_budgeted(LayoutRequest {
            previous: Some(&previous),
            ..request(&projection, &pins, &cancelled, allowance)
        })
        .unwrap();
        assert_eq!(result.evaluations, 1);
        assert_eq!(result.solution.boxes, previous.boxes);
        assert_eq!(result.solution.routes, previous.routes);
        assert!(
            result
                .solution
                .diagnostics
                .last()
                .unwrap()
                .contains("1 accepted-geometry trials, 0 initializations")
        );
    }
}

#[test]
fn initializer_accounting_includes_generated_duplicate_geometry_for_every_algorithm() {
    let projection = fixture();
    let pins = BTreeMap::new();
    let cancelled = AtomicBool::new(false);
    for algorithm in [
        Algorithm::Floorplan,
        Algorithm::Layered,
        Algorithm::Constrained,
        Algorithm::Treemap,
    ] {
        for allowance in [1, 2, 9] {
            let result = solve_budgeted(LayoutRequest {
                algorithm,
                quality: QualityProfile {
                    feedback_candidates: 0,
                    size_candidates: 0,
                    ..Default::default()
                },
                ..request(&projection, &pins, &cancelled, allowance)
            })
            .unwrap();
            assert_eq!(result.evaluations, allowance);
            validate(&projection, &result.solution, &pins, 2).unwrap();
        }
    }
}

#[test]
fn size_fallback_shares_the_allowance_and_preserves_legal_proportion_search() {
    let mut projection = fixture();
    for node in projection.nodes.iter_mut().filter(|node| node.solid) {
        node.min_size = Point::new(10, 10, 10);
        node.max_size = Point::new(60, 60, 60);
    }
    let pins = BTreeMap::from([
        ("a".into(), Point::ZERO),
        ("b".into(), Point::new(24, 0, 0)),
    ]);
    let cancelled = AtomicBool::new(false);
    // The legacy API can exceed one total placement by invoking its independent
    // size allowance. The strict API cannot spend that second evaluation.
    let one = request(&projection, &pins, &cancelled, 1);
    assert!(solve(one).is_ok());
    assert!(solve_budgeted(one).is_err());
    for allowance in [2, 3, 4, 8, 32, 64] {
        let result = solve_budgeted(request(&projection, &pins, &cancelled, allowance)).unwrap();
        assert!(result.evaluations <= allowance);
        assert!(result.evaluations > 1);
        validate(&projection, &result.solution, &pins, 2).unwrap();
        assert!(
            result
                .solution
                .diagnostics
                .iter()
                .any(|message| message.contains("Size-range fallback:"))
        );
        for id in ["a", "b"] {
            assert_eq!(result.solution.boxes[id].bounds.center(), pins[id]);
            if allowance >= 4 {
                // Once both legal variants can be tried, shrink only the axis
                // needed for these pins, rather than choosing tiny cubes.
                assert_eq!(
                    result.solution.boxes[id].bounds.size(),
                    Point::new(10, 40, 40)
                );
            }
        }
    }
    let first = solve_budgeted(request(&projection, &pins, &cancelled, 64)).unwrap();
    projection.nodes.reverse();
    projection.edges.reverse();
    assert_eq!(
        first,
        solve_budgeted(request(&projection, &pins, &cancelled, 64)).unwrap()
    );
}

#[test]
fn measured_feedback_uses_the_shared_remainder_and_improves_a_real_detour() {
    let mut projection = fixture();
    projection.nodes[1] = node("source", Some("root"), Point::new(10, 10, 10));
    projection.nodes[2] = node("target", Some("root"), Point::new(10, 10, 10));
    projection
        .nodes
        .push(node("obstacle", Some("root"), Point::new(20, 30, 14)));
    projection.edges[0].source = "source".into();
    projection.edges[0].target = "target".into();
    let pins = BTreeMap::from([
        ("source".into(), Point::new(-30, 15, 7)),
        ("target".into(), Point::new(80, 15, 7)),
    ]);
    let cancelled = AtomicBool::new(false);
    let static_result = solve_budgeted(LayoutRequest {
        algorithm: Algorithm::Treemap,
        quality: QualityProfile {
            feedback_candidates: 0,
            size_candidates: 0,
            ..Default::default()
        },
        ..request(&projection, &pins, &cancelled, 16)
    })
    .unwrap();
    let dynamic_request = LayoutRequest {
        algorithm: Algorithm::Treemap,
        quality: QualityProfile {
            feedback_candidates: 4,
            size_candidates: 0,
            ..Default::default()
        },
        ..request(&projection, &pins, &cancelled, 32)
    };
    let refined = solve_budgeted(dynamic_request).unwrap();
    assert_eq!(static_result.evaluations, 16);
    assert!(refined.evaluations > 16 && refined.evaluations <= 32);
    assert!(refined.solution.metrics.route_length < static_result.solution.metrics.route_length);
    assert!(
        refined.solution.metrics.structural_cost < static_result.solution.metrics.structural_cost
    );
    validate(&projection, &refined.solution, &pins, 2).unwrap();
    assert!(
        refined
            .solution
            .diagnostics
            .iter()
            .any(|message| message.contains("Routing feedback evaluated"))
    );
    assert_eq!(refined, solve_budgeted(dynamic_request).unwrap());
}

#[test]
fn fixed_geometry_initialization_keeps_existing_cap_without_overflowing_large_allowances() {
    let projection = fixture();
    let pins = BTreeMap::new();
    let cancelled = AtomicBool::new(false);
    let result = solve_budgeted(LayoutRequest {
        quality: QualityProfile {
            feedback_candidates: 0,
            size_candidates: 0,
            ..Default::default()
        },
        ..request(&projection, &pins, &cancelled, usize::MAX)
    })
    .unwrap();
    assert_eq!(result.evaluations, 512);
    validate(&projection, &result.solution, &pins, 2).unwrap();
}
