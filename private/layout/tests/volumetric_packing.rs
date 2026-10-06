use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutRequest, Node, NodeKind, Point, Projection, QualityProfile, solve,
    validate,
};

fn peers(with_upper_band: bool) -> Projection {
    let mut nodes = vec![Node {
        id: "root".into(),
        semantic_ids: vec!["root".into()],
        parent: None,
        kind: NodeKind::Scope,
        band: 0,
        solid: false,
        min_size: Point::new(1, 1, 1),
        max_size: Point::new(10_000, 10_000, 10_000),
        preferred_size: Point::new(36, 24, 36),
    }];
    for i in 0..27 {
        let id = format!("peer:{i:02}");
        nodes.push(Node {
            id: id.clone(),
            semantic_ids: vec![id],
            parent: Some("root".into()),
            kind: NodeKind::Service,
            band: 0,
            solid: true,
            min_size: Point::new(24, 16, 24),
            max_size: Point::new(24, 16, 24),
            preferred_size: Point::new(24, 16, 24),
        });
    }
    if with_upper_band {
        nodes.push(Node {
            id: "auth".into(),
            semantic_ids: vec!["auth".into()],
            parent: Some("root".into()),
            kind: NodeKind::Concern,
            band: 2,
            solid: true,
            min_size: Point::new(24, 6, 24),
            max_size: Point::new(24, 6, 24),
            preferred_size: Point::new(24, 6, 24),
        });
    }
    Projection {
        key: "volumetric-peers".into(),
        model_id: "declared-peers".into(),
        revision: 1,
        root: "root".into(),
        nodes,
        edges: vec![],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    }
}

#[test]
fn many_peers_form_a_compact_volume_without_changing_their_dimensions_or_rank() {
    let projection = peers(false);
    let pins = BTreeMap::new();
    let cancel = AtomicBool::new(false);
    let run = |algorithm| {
        solve(LayoutRequest {
            projection: &projection,
            algorithm,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &cancel,
        })
        .unwrap()
    };
    let floorplan = run(Algorithm::Floorplan);
    let flat = run(Algorithm::Treemap);
    validate(&projection, &floorplan, &pins, 2).unwrap();
    assert!(
        floorplan.metrics.aspect_ratio <= 2.0,
        "expected roughly cubic envelope, got {:?}",
        floorplan.metrics
    );
    assert!(floorplan.metrics.aspect_ratio < flat.metrics.aspect_ratio / 2.0);
    assert!(floorplan.metrics.structural_cost < flat.metrics.structural_cost);
    let heights: std::collections::BTreeSet<_> = projection
        .nodes
        .iter()
        .filter(|n| n.solid)
        .map(|n| {
            let bounds = floorplan.boxes[&n.id].bounds;
            assert_eq!(bounds.size(), n.preferred_size);
            assert_eq!(
                n.band, 0,
                "physical shelves are not new architectural ranks"
            );
            bounds.min.y
        })
        .collect();
    assert!(heights.len() > 1);
}

#[test]
fn volumetric_routes_preserve_upper_bands_pins_and_input_order_independence() {
    let mut projection = peers(true);
    projection.edges = (0..6)
        .map(|i| Edge {
            bundle: None,
            id: format!("edge:{i}"),
            semantic_ids: vec![format!("edge:{i}")],
            source: format!("peer:{:02}", i * 4),
            target: format!("peer:{:02}", i * 4 + 1),
        })
        .collect();
    let pins = BTreeMap::from([("root".into(), Point::new(-100, 0, -100))]);
    let cancel = AtomicBool::new(false);
    let run = |p| {
        solve(LayoutRequest {
            projection: p,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 32,
            cancelled: &cancel,
        })
        .unwrap()
    };
    let solution = run(&projection);
    validate(&projection, &solution, &pins, 2).unwrap();
    assert_eq!(solution.boxes["root"].bounds.center(), pins["root"]);
    let top = solution.boxes["auth"].bounds.min.y;
    for peer in projection
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Service)
    {
        assert!(solution.boxes[&peer.id].bounds.max.y + 12 <= top);
    }
    assert!(solution.metrics.aspect_ratio <= 2.5);
    assert_eq!(solution.routes.len(), 6);
    let mut reversed = projection.clone();
    reversed.nodes.reverse();
    reversed.edges.reverse();
    let reordered = run(&reversed);
    assert_eq!(solution.boxes, reordered.boxes);
    assert_eq!(solution.routes, reordered.routes);
}
