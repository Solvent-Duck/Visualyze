use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use visualyze_layout::{
    Algorithm, Bounds, Edge, LayoutRequest, LayoutSolution, Node, NodeKind, Point, Projection,
    QualityProfile, solve, validate,
};

fn node(id: &str, parent: Option<&str>, kind: NodeKind, band: i32, size: Point) -> Node {
    let solid = kind != NodeKind::Scope;
    Node {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        kind,
        band,
        solid,
        min_size: if solid { size } else { Point::new(1, 1, 1) },
        max_size: if solid {
            size
        } else {
            Point::new(10_000, 10_000, 10_000)
        },
        preferred_size: size,
    }
}

fn projection() -> Projection {
    Projection {
        key: "volumetric-constraints".into(),
        model_id: "declared-volumetric-constraints".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![node(
            "root",
            None,
            NodeKind::Scope,
            0,
            Point::new(36, 24, 36),
        )],
        edges: vec![],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    }
}

fn peers(projection: &mut Projection, prefix: &str, count: usize, band: i32) {
    projection.nodes.extend((0..count).map(|i| {
        node(
            &format!("{prefix}:{i:02}"),
            Some("root"),
            NodeKind::Service,
            band,
            Point::new(24, 16, 24),
        )
    }));
}

fn edge(id: &str, source: &str, target: &str) -> Edge {
    Edge {
        bundle: None,
        id: id.into(),
        semantic_ids: vec![id.into()],
        source: source.into(),
        target: target.into(),
    }
}

fn run(projection: &Projection, pins: &BTreeMap<String, Point>) -> LayoutSolution {
    let cancelled = AtomicBool::new(false);
    let result = solve(LayoutRequest {
        projection,
        algorithm: Algorithm::Floorplan,
        pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 48,
        cancelled: &cancelled,
    })
    .unwrap();
    validate(projection, &result, pins, 2).unwrap();
    assert_eq!(result.routes.len(), projection.edges.len());
    result
}

fn assert_stacked(projection: &Projection, solution: &LayoutSolution, prefix: &str) {
    let heights: BTreeSet<_> = projection
        .nodes
        .iter()
        .filter(|node| node.id.starts_with(prefix))
        .map(|node| solution.boxes[&node.id].bounds.min.y)
        .collect();
    assert!(
        heights.len() > 1,
        "{prefix} must remain volumetric, not pass by falling back to a plane"
    );
}

fn assert_vertical_route(solution: &LayoutSolution, edge_id: &str, upper: &str, lower: &str) {
    let route = solution
        .routes
        .iter()
        .find(|route| route.edge_id == edge_id)
        .unwrap();
    let high = solution.boxes[upper].bounds;
    let low = solution.boxes[lower].bounds;
    assert_eq!(
        route.points.len(),
        2,
        "expected a direct vertical passage: {route:?}"
    );
    assert_eq!(route.points[0].x, route.points[1].x);
    assert_eq!(route.points[0].z, route.points[1].z);
    assert_eq!(route.points[0].y, high.min.y);
    assert_eq!(route.points[1].y, low.max.y);
    let shaft = Bounds {
        min: Point::new(route.points[0].x - 2, low.max.y, route.points[0].z - 2),
        max: Point::new(route.points[0].x + 2, high.min.y, route.points[0].z + 2),
    };
    for object in solution
        .boxes
        .values()
        .filter(|b| b.solid && b.id != upper && b.id != lower)
    {
        assert!(
            !shaft.overlaps(object.bounds),
            "{} obstructs the reserved passage",
            object.id
        );
    }
}

#[test]
fn auth_reaches_lowest_id_peer_directly_without_flattening_its_band() {
    let mut projection = projection();
    peers(&mut projection, "peer", 27, 0);
    projection.nodes.push(node(
        "auth",
        Some("root"),
        NodeKind::Concern,
        2,
        Point::new(24, 6, 24),
    ));
    projection
        .edges
        .push(edge("auth-coverage", "auth", "peer:00"));
    let solution = run(&projection, &BTreeMap::new());
    assert_stacked(&projection, &solution, "peer:");
    assert!(solution.metrics.aspect_ratio <= 2.0);
    assert_vertical_route(&solution, "auth-coverage", "auth", "peer:00");
    assert_eq!(
        solution.metrics.route_length, 12,
        "the covered peer should be on the upper physical shelf"
    );
    assert_eq!(solution.metrics.bends, 0);
}

#[test]
fn skipped_band_passage_clears_stacked_endpoint_and_intermediate_peers() {
    let mut projection = projection();
    peers(&mut projection, "data", 27, 0);
    peers(&mut projection, "middleware", 27, 1);
    projection.nodes.push(node(
        "waitlist",
        Some("root"),
        NodeKind::Client,
        2,
        Point::new(24, 6, 24),
    ));
    projection
        .edges
        .push(edge("waitlist-write", "waitlist", "data:00"));
    let solution = run(&projection, &BTreeMap::new());
    assert_stacked(&projection, &solution, "data:");
    assert_stacked(&projection, &solution, "middleware:");
    assert_vertical_route(&solution, "waitlist-write", "waitlist", "data:00");
    let data_top = projection
        .nodes
        .iter()
        .filter(|node| node.id.starts_with("data:"))
        .map(|node| solution.boxes[&node.id].bounds.max.y)
        .max()
        .unwrap();
    let middleware_bottom = projection
        .nodes
        .iter()
        .filter(|node| node.id.starts_with("middleware:"))
        .map(|node| solution.boxes[&node.id].bounds.min.y)
        .min()
        .unwrap();
    let middleware_top = projection
        .nodes
        .iter()
        .filter(|node| node.id.starts_with("middleware:"))
        .map(|node| solution.boxes[&node.id].bounds.max.y)
        .max()
        .unwrap();
    assert!(data_top + 12 <= middleware_bottom);
    assert!(middleware_top + 12 <= solution.boxes["waitlist"].bounds.min.y);
}

#[test]
fn a_solid_peer_pin_is_preserved_in_a_stacked_solution() {
    let mut projection = projection();
    peers(&mut projection, "peer", 27, 0);
    projection
        .edges
        .push(edge("peer-dependency", "peer:00", "peer:01"));
    let initial = run(&projection, &BTreeMap::new());
    assert_stacked(&projection, &initial, "peer:");
    let original = initial.boxes["peer:00"].bounds.center();
    let pins = BTreeMap::from([("peer:00".into(), original + Point::new(3, 0, 0))]);
    let pinned = run(&projection, &pins);
    assert_eq!(pinned.boxes["peer:00"].bounds.center(), pins["peer:00"]);
    assert_ne!(pinned.boxes["peer:00"].bounds.center(), original);
    assert_stacked(&projection, &pinned, "peer:");
    for node in projection.nodes.iter().filter(|node| node.solid) {
        assert_eq!(pinned.boxes[&node.id].bounds.size(), node.preferred_size);
    }
    projection.nodes.reverse();
    projection.edges.reverse();
    let reordered = run(&projection, &pins);
    assert_eq!(pinned.boxes, reordered.boxes);
    assert_eq!(pinned.routes, reordered.routes);
}

#[test]
fn eight_services_pack_full_attached_interface_envelopes_on_multiple_shelves() {
    let mut projection = projection();
    peers(&mut projection, "service", 8, 0);
    for i in 0..8 {
        projection.nodes.push(node(
            &format!("interface:{i:02}"),
            Some(&format!("service:{i:02}")),
            NodeKind::Interface,
            0,
            Point::new(8, 4, 4),
        ));
    }
    for i in (0..8).step_by(2) {
        projection.edges.push(edge(
            &format!("request:{i:02}"),
            &format!("service:{i:02}"),
            &format!("interface:{:02}", i + 1),
        ));
    }
    let solution = run(&projection, &BTreeMap::new());
    assert_stacked(&projection, &solution, "service:");
    for i in 0..8 {
        let service = solution.boxes[&format!("service:{i:02}")].bounds;
        let pad = solution.boxes[&format!("interface:{i:02}")].bounds;
        assert_eq!(pad.min.z, service.max.z);
        assert!(pad.min.x >= service.min.x && pad.max.x <= service.max.x);
        assert!(pad.min.y >= service.min.y && pad.max.y <= service.max.y);
        assert_eq!(pad.size(), Point::new(8, 4, 4));
        for j in 0..8 {
            if i != j {
                assert!(!pad.overlaps(solution.boxes[&format!("service:{j:02}")].bounds));
                assert!(!pad.overlaps(solution.boxes[&format!("interface:{j:02}")].bounds));
            }
        }
    }
}
