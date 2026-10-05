use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    composed::{
        CanonicalPins, ComposedRequest, ComposedSolution, solve_composed, validate_composed,
    },
    projection::{ViewState, project},
    provider_projection::{ProviderViewRequest, project_provider},
};
use visualyze_model::{
    architecture::Architecture,
    provider::{Limits, LocalBundle},
};

fn node(id: &str, parent: Option<&str>, kind: NodeKind, solid: bool, band: i32) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![format!("semantic:{id}")],
        parent: parent.map(str::to_owned),
        kind,
        solid,
        band,
        min_size: Point::new(12, 8, 12),
        preferred_size: Point::new(24, 16, 24),
        max_size: Point::new(48, 32, 48),
    }
}
fn edge(id: &str, source: &str, target: &str) -> Edge {
    Edge {
        id: id.into(),
        source: source.into(),
        target: target.into(),
        semantic_ids: vec![format!("claim:{id}")],
    }
}
fn nested() -> Projection {
    Projection {
        key: "nested-v1".into(),
        model_id: "declared-nested".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, NodeKind::Scope, false, 0),
            node("auth", Some("root"), NodeKind::Concern, true, 3),
            node("db", Some("root"), NodeKind::Datastore, false, 0),
            node("table", Some("db"), NodeKind::Group, false, 0),
            node("row", Some("table"), NodeKind::Service, true, 0),
            node("peer", Some("root"), NodeKind::Service, true, 1),
        ],
        edges: vec![edge("auth-row", "auth", "row")],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    }
}
fn run(p: &Projection, algorithm: Algorithm, pins: &CanonicalPins) -> ComposedSolution {
    solve_composed(ComposedRequest {
        projection: p,
        algorithm,
        quality: QualityProfile::default(),
        pins,
        work_budget: 192,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap()
}

#[test]
fn all_initializers_solve_complete_nested_relationships_with_exact_presentation() {
    let p = nested();
    for algorithm in [
        Algorithm::Floorplan,
        Algorithm::Layered,
        Algorithm::Constrained,
        Algorithm::Treemap,
    ] {
        let result = run(&p, algorithm, &CanonicalPins::new());
        validate_composed(&p, &result, &CanonicalPins::new(), &AtomicBool::new(false)).unwrap();
        let route = &result.routes["auth-row"];
        assert_eq!(route.edge, p.edges[0]);
        assert_eq!(
            route
                .fragments
                .iter()
                .map(|f| f.frame_id.as_str())
                .collect::<Vec<_>>(),
            ["root", "db", "table"]
        );
        let presentation = result.present(&AtomicBool::new(false)).unwrap();
        assert_eq!(
            presentation.boxes.keys().collect::<BTreeSet<_>>(),
            p.nodes.iter().map(|n| &n.id).collect()
        );
        for node in &p.nodes {
            assert_eq!(presentation.boxes[&node.id].parent, node.parent);
        }
        assert!(!presentation.boxes["db"].solid);
        assert!(!presentation.boxes["table"].solid);
        assert_eq!(presentation.routes.len(), 1);
        assert_eq!(
            presentation.routes[0]
                .scope_ports
                .iter()
                .map(|port| (port.assembly_id.as_str(), port.entering))
                .collect::<Vec<_>>(),
            [("db", true), ("table", true)]
        );
        assert!(
            result.work.evaluations.values().sum::<usize>() + result.work.feedback_evaluations
                <= result.work.budget
        );
        assert!(result.metrics.maximum_scale_shift > 0);
        let actual: i64 = route
            .fragments
            .iter()
            .flat_map(|f| f.route.points.windows(2))
            .map(|p| p[0].manhattan(p[1]))
            .sum();
        assert_eq!(result.metrics.canonical_route_length, actual);
        assert!(result.metrics.presented_route_length < actual as f64);
        assert_eq!(
            result.metrics.structural_cost,
            result
                .metrics
                .frames
                .values()
                .map(|m| m.structural_cost)
                .sum::<f64>()
        );
    }
}

#[test]
fn unobstructed_auth_to_nested_middleware_stays_vertical_through_every_frame() {
    let mut p = nested();
    p.nodes.retain(|node| node.id != "peer");
    let result = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    let presented = result.present(&AtomicBool::new(false)).unwrap();
    let points = &presented.routes[0].points;
    let first = points[0].to_f64();
    assert!(
        points.iter().all(|point| {
            let p = point.to_f64();
            p[0] == first[0] && p[2] == first[2]
        }),
        "{:?}",
        points.iter().map(|p| p.to_f64()).collect::<Vec<_>>()
    );
    assert_eq!(result.metrics.bends, 0);
}

#[test]
fn surface_endpoints_stop_at_their_real_boundary_and_ancestor_edges_enter_the_interior() {
    let mut p = nested();
    p.edges = vec![
        edge("outside-db", "auth", "db"),
        edge("table-outside", "table", "peer"),
        edge("db-row", "db", "row"),
        edge("row-db", "row", "db"),
    ];
    let result = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    for (edge, frames) in [
        ("outside-db", vec!["root"]),
        ("table-outside", vec!["db", "root"]),
        ("db-row", vec!["db", "table"]),
        ("row-db", vec!["table", "db"]),
    ] {
        assert_eq!(
            result.routes[edge]
                .fragments
                .iter()
                .map(|f| f.frame_id.as_str())
                .collect::<Vec<_>>(),
            frames
        );
    }
}

#[test]
fn parallel_cycles_and_input_reordering_keep_claims_and_geometry() {
    let mut p = nested();
    p.edges.extend([
        edge("auth-row-2", "auth", "row"),
        edge("row-auth", "row", "auth"),
        edge("peer-row", "peer", "row"),
        edge("row-peer", "row", "peer"),
    ]);
    let first = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    p.nodes.reverse();
    p.edges.reverse();
    let reordered = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    assert_eq!(
        first.present(&AtomicBool::new(false)).unwrap(),
        reordered.present(&AtomicBool::new(false)).unwrap()
    );
    assert_eq!(first.routes, reordered.routes);
    assert_eq!(first.metrics, reordered.metrics);
    assert_eq!(first.work, reordered.work);
    assert_eq!(first.routes.len(), p.edges.len());
}

#[test]
fn descendant_growth_does_not_enlarge_or_move_the_parent_proxy() {
    let mut p = nested();
    p.edges.clear();
    let first = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    p.nodes.extend((0..18).map(|i| {
        node(
            &format!("column-{i:02}"),
            Some("table"),
            NodeKind::Service,
            true,
            0,
        )
    }));
    let grown = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    for frame in ["root", "db"] {
        assert_eq!(
            first.assembly.frame(frame).unwrap().solution().boxes,
            grown.assembly.frame(frame).unwrap().solution().boxes
        );
    }
    assert!(
        grown
            .assembly
            .instance("table")
            .unwrap()
            .transform
            .scale_shift()
            > first
                .assembly
                .instance("table")
                .unwrap()
                .transform
                .scale_shift()
    );
}

#[test]
fn canonical_root_and_leaf_pins_survive_envelope_padding() {
    let p = nested();
    let pins = BTreeMap::from([(
        "table".into(),
        BTreeMap::from([
            ("table".into(), Point::new(-100, 30, 50)),
            ("row".into(), Point::new(-100, 30, 50)),
        ]),
    )]);
    let result = run(&p, Algorithm::Floorplan, &pins);
    for (id, pin) in &pins["table"] {
        assert_eq!(
            result.assembly.frame("table").unwrap().solution().boxes[id]
                .bounds
                .center(),
            *pin
        );
    }
    validate_composed(&p, &result, &pins, &AtomicBool::new(false)).unwrap();
}

#[test]
fn insufficient_global_budget_bad_pins_depth_and_cancellation_reject_transactionally() {
    let mut p = nested();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let request = ComposedRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &pins,
        work_budget: 2,
        cancelled: &cancel,
    };
    assert!(
        solve_composed(request)
            .unwrap_err()
            .to_string()
            .contains("work budget")
    );
    assert!(matches!(
        solve_composed(ComposedRequest {
            work_budget: 192,
            cancelled: &AtomicBool::new(true),
            ..request
        }),
        Err(LayoutError::Cancelled)
    ));
    let bad = BTreeMap::from([("unknown-frame".into(), BTreeMap::new())]);
    assert!(
        solve_composed(ComposedRequest {
            pins: &bad,
            work_budget: 192,
            ..request
        })
        .is_err()
    );
    p.nodes = vec![node("root", None, NodeKind::Scope, false, 0)];
    p.edges.clear();
    for i in 0..33 {
        p.nodes.push(node(
            &format!("depth-{i}"),
            Some(&if i == 0 {
                "root".into()
            } else {
                format!("depth-{}", i - 1)
            }),
            NodeKind::Group,
            false,
            0,
        ));
    }
    assert!(
        solve_composed(ComposedRequest {
            projection: &p,
            work_budget: 192,
            pins: &pins,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            cancelled: &cancel
        })
        .unwrap_err()
        .to_string()
        .contains("depth")
    );
}

#[test]
fn real_snapshot_and_typed_database_expansion_solve_with_original_memberships() {
    let model = Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
        .unwrap();
    let p = project(&model, &ViewState::default()).unwrap();
    let result = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    assert_eq!(
        result.present(&AtomicBool::new(false)).unwrap().boxes.len(),
        p.nodes.len()
    );
    let mut focused_view = ViewState::default();
    focused_view.focus = Some("service:notes".into());
    let focus = project(&model, &focused_view).unwrap();
    let focused = run(&focus, Algorithm::Floorplan, &CanonicalPins::new());
    assert_eq!(
        focused
            .present(&AtomicBool::new(false))
            .unwrap()
            .boxes
            .len(),
        focus.nodes.len()
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../public/examples/data-messaging.bundle.jsonl");
    let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
    let p = project_provider(
        &mut provider,
        &ProviderViewRequest {
            expanded: BTreeSet::from(["datastore:orders".into(), "table:orders".into()]),
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap()
    .graph;
    let result = run(&p, Algorithm::Floorplan, &CanonicalPins::new());
    assert_eq!(result.routes.len(), p.edges.len());
    assert_eq!(
        result.present(&AtomicBool::new(false)).unwrap().boxes.len(),
        p.nodes.len()
    );
}
