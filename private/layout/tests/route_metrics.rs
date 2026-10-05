use std::collections::BTreeMap;
use visualyze_layout::{
    Bounds, Edge, LAYOUT_VERSION, LayoutBox, LayoutSolution, Metrics, Node, NodeKind, Point,
    Projection, Route, measure, metrics::route_congestion, validate,
};

fn p(x: i64, y: i64, z: i64) -> Point {
    Point::new(x, y, z)
}
fn route(id: &str, points: Vec<Point>) -> Route {
    Route {
        edge_id: id.into(),
        semantic_ids: vec![id.into()],
        points,
        scope_ports: vec![],
    }
}

#[test]
fn collinear_occupancy_counts_distinct_routes_and_not_explicit_bundle_members() {
    let mut routes = vec![
        route("a", vec![p(0, 0, 0), p(10, 0, 0)]),
        route("b", vec![p(5, 0, 0), p(15, 0, 0)]),
        route("c", vec![p(8, 0, 0), p(12, 0, 0)]),
    ];
    let measured = route_congestion(&routes);
    assert_eq!(measured.shared_lane_length, 9); // 3 + (2 * 2) + 2
    assert_eq!(measured.crossing_points, 0);
    routes[0].semantic_ids = (0..100).map(|i| format!("bundled:{i}")).collect();
    assert_eq!(route_congestion(&routes), measured);
    routes[0].points = vec![p(0, 0, 0), p(4, 0, 0), p(10, 0, 0)];
    routes[1].points.reverse();
    routes.reverse();
    assert_eq!(
        route_congestion(&routes),
        measured,
        "scope-port splits and order do not multiply sharing"
    );
    assert_eq!(
        route_congestion(&[route("self", vec![p(0, 0, 0), p(10, 0, 0), p(0, 0, 0)])])
            .shared_lane_length,
        0
    );
}

#[test]
fn perpendicular_intersections_are_exact_3d_locations_not_nearby_lanes() {
    let routes = vec![
        route("x", vec![p(-4, 0, 0), p(0, 0, 0), p(4, 0, 0)]),
        route("y", vec![p(0, -4, 0), p(0, 4, 0)]),
        route("z", vec![p(0, 0, -4), p(0, 0, 4)]),
        route("separate-plane", vec![p(-4, -4, 1), p(4, -4, 1)]),
    ];
    assert_eq!(
        route_congestion(&routes).crossing_points,
        1,
        "a three-route junction is one geometric location"
    );
    assert_eq!(route_congestion(&routes).shared_lane_length, 0);
    let mut reordered = routes.clone();
    reordered.reverse();
    for route in &mut reordered {
        route.points.reverse();
    }
    assert_eq!(route_congestion(&reordered), route_congestion(&routes));
    let parallel = [
        route("x", vec![p(0, 0, 0), p(10, 0, 0)]),
        route("near", vec![p(0, 1, 0), p(10, 1, 0)]),
    ];
    assert_eq!(
        route_congestion(&parallel),
        Default::default(),
        "this exact-track metric makes no tube-clearance claim"
    );
}

#[test]
fn intentional_common_attachments_are_exempt_but_interior_junctions_are_not() {
    let common = [
        route("a", vec![p(-4, 0, 0), p(0, 0, 0)]),
        route("b", vec![p(0, 0, 0), p(0, 4, 0)]),
    ];
    assert_eq!(route_congestion(&common), Default::default());
    let mut interior = common.to_vec();
    interior[0].points.push(p(4, 0, 0));
    assert_eq!(route_congestion(&interior).crossing_points, 1);
    let bend = [
        route("bend", vec![p(-4, 0, 0), p(0, 0, 0), p(0, 4, 0)]),
        route("cross", vec![p(0, 0, -4), p(0, 0, 4)]),
    ];
    assert_eq!(
        route_congestion(&bend).crossing_points,
        1,
        "a crossing at a polyline bend is not counted twice"
    );
}

fn crossing_fixture() -> (Projection, LayoutSolution) {
    let shapes = [
        ("a", p(-12, -2, -2), p(-8, 2, 2)),
        ("b", p(8, -2, -2), p(12, 2, 2)),
        ("c", p(-2, -12, -2), p(2, -8, 2)),
        ("d", p(-2, 8, -2), p(2, 12, 2)),
    ];
    let nodes = shapes
        .iter()
        .map(|(id, min, max)| Node {
            id: (*id).into(),
            semantic_ids: vec![(*id).into()],
            parent: None,
            kind: NodeKind::Service,
            band: 0,
            min_size: *max - *min,
            max_size: *max - *min,
            preferred_size: *max - *min,
            solid: true,
        })
        .collect();
    let edges = [("x", "a", "b"), ("y", "c", "d")]
        .map(|(id, source, target)| Edge {
            id: id.into(),
            semantic_ids: vec![id.into()],
            source: source.into(),
            target: target.into(),
        })
        .to_vec();
    let projection = Projection {
        key: "metric-fixture".into(),
        model_id: "metric-fixture".into(),
        revision: 1,
        root: "a".into(),
        nodes,
        edges,
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let solution = LayoutSolution {
        model_id: projection.model_id.clone(),
        revision: 1,
        projection_key: projection.key.clone(),
        version: LAYOUT_VERSION,
        boxes: shapes
            .into_iter()
            .map(|(id, min, max)| {
                (
                    id.into(),
                    LayoutBox {
                        id: id.into(),
                        semantic_ids: vec![id.into()],
                        parent: None,
                        bounds: Bounds { min, max },
                        solid: true,
                    },
                )
            })
            .collect(),
        routes: vec![
            route("x", vec![p(-8, 0, 0), p(8, 0, 0)]),
            route("y", vec![p(0, -8, 0), p(0, 8, 0)]),
        ],
        metrics: Metrics::default(),
        diagnostics: vec![],
    };
    (projection, solution)
}

#[test]
fn actual_valid_reroute_removes_congestion_and_scoring_uses_a_fixed_projection_scale() {
    let (projection, original) = crossing_fixture();
    let mut rerouted = original.clone();
    rerouted.routes[1].points = vec![p(0, -8, 0), p(0, -8, 4), p(0, 8, 4), p(0, 8, 0)];
    for solution in [&original, &rerouted] {
        validate(&projection, solution, &BTreeMap::new(), 2).unwrap();
    }
    let before = measure(&projection, &original, None);
    let after = measure(&projection, &rerouted, None);
    assert_eq!(before.crossing_points, 1);
    assert_eq!(before.congestion, 0.5);
    assert_eq!(after.congestion, 0.0);
    let scale = (4.0_f64 * 4.0 * 4.0 * 4.0).cbrt();
    let expected_cost_change = 8.0 / scale + 0.6 - 0.5;
    assert!((after.structural_cost - before.structural_cost - expected_cost_change).abs() < 1e-10);
    let mut shared = original.clone();
    shared.routes[1].points = original.routes[0].points.clone();
    let metric = measure(&projection, &shared, None);
    assert_eq!(metric.shared_lane_length, 16);
    assert!((metric.congestion - 16.0 / scale).abs() < 1e-10);
    let mut resized = shared.clone();
    resized.boxes.get_mut("a").unwrap().bounds.min.x -= 10;
    assert_eq!(
        measure(&projection, &resized, None).congestion,
        metric.congestion,
        "candidate resize cannot improve the denominator"
    );
}

#[test]
fn four_thousand_routes_on_distinct_planes_are_measured_without_pairwise_crossings() {
    let routes: Vec<_> = (0..4096)
        .map(|i| {
            route(
                &format!("route:{i}"),
                vec![p(0, 0, i), p(100, 0, i), p(100, 100, i)],
            )
        })
        .collect();
    assert_eq!(route_congestion(&routes), Default::default());
}

#[test]
fn legacy_metrics_deserialize_with_zero_congestion_fields() {
    let mut json = serde_json::to_value(Metrics::default()).unwrap();
    for name in ["shared_lane_length", "crossing_points", "congestion"] {
        json.as_object_mut().unwrap().remove(name);
    }
    let old: Metrics = serde_json::from_value(json).unwrap();
    assert_eq!(old.shared_lane_length, 0);
    assert_eq!(old.crossing_points, 0);
    assert_eq!(old.congestion, 0.0);
}

#[test]
fn indexed_sweeps_match_an_independent_small_integer_lattice_oracle() {
    use std::collections::BTreeSet;
    let mut seed = 7_u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as i64
    };
    for _ in 0..16 {
        let mut routes = Vec::new();
        for id in 0..12 {
            let mut current = [next() % 5, next() % 5, next() % 5];
            let mut points = vec![p(current[0], current[1], current[2])];
            for _ in 0..4 {
                let axis = (next() % 3) as usize;
                current[axis] = (current[axis] + 1 + next() % 4) % 5;
                points.push(p(current[0], current[1], current[2]));
            }
            routes.push(route(&format!("r:{id}"), points));
        }
        // Unit edges count occupied lattice lengths, independent of line sweeps.
        let mut unit_edges = BTreeMap::<(Point, Point), BTreeSet<usize>>::new();
        let mut at_point = BTreeMap::<Point, BTreeMap<usize, BTreeSet<usize>>>::new();
        for (id, route) in routes.iter().enumerate() {
            for segment in route.points.windows(2) {
                let a = [segment[0].x, segment[0].y, segment[0].z];
                let b = [segment[1].x, segment[1].y, segment[1].z];
                let axis = (0..3).find(|axis| a[*axis] != b[*axis]).unwrap();
                let (lo, hi) = (a[axis].min(b[axis]), a[axis].max(b[axis]));
                for coordinate in lo..=hi {
                    let mut position = a;
                    position[axis] = coordinate;
                    let point = p(position[0], position[1], position[2]);
                    at_point
                        .entry(point)
                        .or_default()
                        .entry(id)
                        .or_default()
                        .insert(axis);
                    if coordinate < hi {
                        position[axis] += 1;
                        unit_edges
                            .entry((point, p(position[0], position[1], position[2])))
                            .or_default()
                            .insert(id);
                    }
                }
            }
        }
        let shared = unit_edges
            .values()
            .map(|routes| routes.len().saturating_sub(1) as i64)
            .sum::<i64>();
        let crosses = at_point
            .iter()
            .filter(|(point, routes_at_point)| {
                routes_at_point.iter().any(|(a, axes_a)| {
                    routes_at_point.iter().any(|(b, axes_b)| {
                        if a >= b {
                            return false;
                        }
                        let both_endpoints = [*a, *b].iter().all(|id| {
                            routes[*id].points.first() == Some(*point)
                                || routes[*id].points.last() == Some(*point)
                        });
                        !both_endpoints
                            && axes_a
                                .iter()
                                .any(|axis_a| axes_b.iter().any(|axis_b| axis_a != axis_b))
                    })
                })
            })
            .count();
        let metric = route_congestion(&routes);
        assert_eq!(metric.shared_lane_length, shared);
        assert_eq!(metric.crossing_points, crosses);
    }
}
