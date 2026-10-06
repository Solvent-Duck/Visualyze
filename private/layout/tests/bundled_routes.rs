//! Lines that cross an expanded box share one trunk in the parent and fan out
//! only inside the box (ADR 0007).
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

use visualyze_layout::{
    Algorithm, LayoutRequest, LayoutSolution, Point, Projection, QualityProfile, Route,
    composed::{CanonicalPins, ComposedRequest, solve_composed, validate_composed},
    projection::{ViewState, project},
    solve, validate,
};
use visualyze_model::architecture::Architecture;

/// Scope `sa` (a1..a3) and scope `sc` (c1, c2) beside outside service `sb`.
fn model() -> Architecture {
    let entity = |id: &str, kind: &str, extra: &str| {
        format!(
            r#"{{"id":"{id}","kind":"{kind}","label":"{id}","boundary":"internal","basis":"declared"{extra}}}"#
        )
    };
    let relation = |id: &str, kind: &str, source: &str, target: &str| {
        format!(
            r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"}}"#
        )
    };
    let entities = [
        entity("system", "scope", r#","subtype":"system""#),
        entity("sa", "scope", ""),
        entity("sc", "scope", ""),
        entity("a1", "service", ""),
        entity("a2", "service", ""),
        entity("a3", "service", ""),
        entity("c1", "service", ""),
        entity("c2", "service", ""),
        entity("sb", "service", ""),
    ];
    let relations = [
        relation("own-sa", "contains", "system", "sa"),
        relation("own-sc", "contains", "system", "sc"),
        relation("own-sb", "contains", "system", "sb"),
        relation("own-a1", "contains", "sa", "a1"),
        relation("own-a2", "contains", "sa", "a2"),
        relation("own-a3", "contains", "sa", "a3"),
        relation("own-c1", "contains", "sc", "c1"),
        relation("own-c2", "contains", "sc", "c2"),
        relation("b1", "requests", "a1", "sb"),
        relation("b2", "requests", "a2", "sb"),
        relation("b3", "requests", "a3", "sb"),
        relation("x1", "requests", "a1", "c1"),
        relation("x2", "requests", "a2", "c2"),
        relation("x3", "requests", "a3", "c1"),
    ];
    let json = format!(
        r#"{{"schema_version":"0.1.0-draft.1","model_id":"bundled-routes","revision":1,"system_id":"system","snapshot":{{"id":"bundled-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
        entities.join(","),
        relations.join(",")
    );
    Architecture::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
}

fn expanded(ids: &[&str]) -> ViewState {
    let mut view = ViewState::default();
    for id in ids {
        view.set_expanded(*id, true);
    }
    view
}

fn layout(projection: &Projection, algorithm: Algorithm) -> LayoutSolution {
    let pins = BTreeMap::new();
    let solution = solve(LayoutRequest {
        projection,
        algorithm,
        pins: &pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 32,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    validate(projection, &solution, &pins, 2).unwrap();
    solution
}

fn segments(route: &Route) -> BTreeSet<(Point, Point)> {
    route
        .points
        .windows(2)
        .map(|pair| (pair[0].min(pair[1]), pair[0].max(pair[1])))
        .collect()
}

/// Segments shared by every member route of `relation_ids`' bundle.
fn shared_trunk(
    projection: &Projection,
    solution: &LayoutSolution,
    relation_ids: &[&str],
) -> BTreeSet<(Point, Point)> {
    let routes: Vec<_> = relation_ids
        .iter()
        .map(|id| {
            let edge = projection
                .edges
                .iter()
                .find(|edge| edge.semantic_ids.iter().any(|member| member == id))
                .unwrap();
            assert!(edge.bundle.is_some(), "{id} crosses an expanded box");
            solution
                .routes
                .iter()
                .find(|route| route.edge_id == edge.id)
                .unwrap()
        })
        .collect();
    let mut shared = segments(routes[0]);
    for route in &routes[1..] {
        shared = shared.intersection(&segments(route)).copied().collect();
    }
    shared
}

fn outside(point: Point, frame: &LayoutSolution, id: &str) -> bool {
    let bounds = frame.boxes[id].bounds;
    point.x < bounds.min.x
        || point.x > bounds.max.x
        || point.y < bounds.min.y
        || point.y > bounds.max.y
        || point.z < bounds.min.z
        || point.z > bounds.max.z
}

#[test]
fn lines_leaving_an_expanded_box_share_one_trunk_outside_it() {
    let model = model();
    let projection = project(&model, &expanded(&["sa"])).unwrap();
    for algorithm in [
        Algorithm::Floorplan,
        Algorithm::Layered,
        Algorithm::Constrained,
    ] {
        let solution = layout(&projection, algorithm);
        let trunk = shared_trunk(&projection, &solution, &["b1", "b2", "b3"]);
        assert!(
            !trunk.is_empty(),
            "{algorithm:?}: a1..a3 -> sb share a trunk"
        );
        // The shared part is the trunk outside sa; the fan-out stays inside.
        assert!(
            trunk
                .iter()
                .any(|(a, b)| outside(*a, &solution, "sa") || outside(*b, &solution, "sa")),
            "{algorithm:?}: the trunk runs in the parent region"
        );
    }
}

#[test]
fn lines_between_two_expanded_boxes_share_one_trunk_between_them() {
    let model = model();
    let projection = project(&model, &expanded(&["sa", "sc"])).unwrap();
    let solution = layout(&projection, Algorithm::Floorplan);
    let trunk = shared_trunk(&projection, &solution, &["x1", "x2", "x3"]);
    assert!(
        trunk.iter().any(|(a, b)| {
            let middle = Point::new((a.x + b.x) / 2, (a.y + b.y) / 2, (a.z + b.z) / 2);
            outside(middle, &solution, "sa") && outside(middle, &solution, "sc")
        }),
        "x1..x3 share a segment between sa and sc"
    );
}

#[test]
fn bundling_never_changes_semantic_membership() {
    let model = model();
    let projection = project(&model, &expanded(&["sa", "sc"])).unwrap();
    let solution = layout(&projection, Algorithm::Floorplan);
    let routed: BTreeSet<_> = solution
        .routes
        .iter()
        .flat_map(|route| route.semantic_ids.iter().cloned())
        .collect();
    let projected: BTreeSet<_> = projection
        .edges
        .iter()
        .flat_map(|edge| edge.semantic_ids.iter().cloned())
        .collect();
    assert_eq!(routed, projected);
    assert_eq!(solution.routes.len(), projection.edges.len());
}

#[test]
fn the_composed_engine_shares_one_trunk_fragment_per_bundle() {
    let model = model();
    let projection = project(&model, &expanded(&["sa", "sc"])).unwrap();
    let pins = CanonicalPins::new();
    let cancelled = AtomicBool::new(false);
    let solution = solve_composed(ComposedRequest {
        projection: &projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &pins,
        work_budget: 192,
        cancelled: &cancelled,
    })
    .unwrap();
    validate_composed(&projection, &solution, &pins, &cancelled).unwrap();
    for members in [["b1", "b2", "b3"], ["x1", "x2", "x3"]] {
        let edges: Vec<_> = members
            .iter()
            .map(|id| {
                projection
                    .edges
                    .iter()
                    .find(|edge| edge.semantic_ids.iter().any(|member| member == id))
                    .unwrap()
            })
            .collect();
        let key = edges[0].bundle.clone().expect("bundled");
        let trunks: Vec<_> = edges
            .iter()
            .map(|edge| {
                solution.routes[&edge.id]
                    .fragments
                    .iter()
                    .find(|fragment| fragment.request.id == key)
                    .expect("each member carries the shared trunk")
                    .route
                    .points
                    .clone()
            })
            .collect();
        assert!(
            trunks.windows(2).all(|pair| pair[0] == pair[1]),
            "{members:?}"
        );
    }
    // Presented members overlap exactly along the trunk.
    let presented = solution.present(&cancelled).unwrap();
    assert_eq!(presented.routes.len(), projection.edges.len());
}
