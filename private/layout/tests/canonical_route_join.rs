//! Cross-module proof: a real parent route chooses the boundary attachment;
//! its exact inverse is the child's fixed terminal. This does not replace the
//! future whole-assembly relationship/fragment completeness validator.
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Bounds, Edge, LAYOUT_VERSION, LayoutBox, LayoutSolution, Metrics, Point,
    assembly::{Assembly, AssemblyInstance, CanonicalFrame, DyadicTransform},
    routing::{FragmentRequest, FragmentTerminal, route_all, route_fragments, validate_fragments},
};

fn bounds(min: [i64; 3], max: [i64; 3]) -> Bounds {
    Bounds {
        min: Point::new(min[0], min[1], min[2]),
        max: Point::new(max[0], max[1], max[2]),
    }
}

fn object(id: &str, parent: Option<&str>, bounds: Bounds, solid: bool) -> LayoutBox {
    LayoutBox {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        bounds,
        solid,
    }
}

fn frame(boxes: BTreeMap<String, LayoutBox>) -> CanonicalFrame {
    CanonicalFrame::new(LayoutSolution {
        model_id: "declared-canonical-join".into(),
        revision: 1,
        projection_key: "declared-canonical-join-v1".into(),
        version: LAYOUT_VERSION,
        boxes,
        // Boundary fragments are not ordinary kernel edges: they are retained
        // separately below, without inventing semantic endpoint boxes.
        routes: vec![],
        metrics: Metrics::default(),
        diagnostics: vec![],
    })
    .unwrap()
}

fn fixture(block_vertical_passage: bool) {
    let cancelled = AtomicBool::new(false);
    let parent_domain = bounds([0, 0, 0], [100, 100, 100]);
    let envelope = bounds([20, 10, 20], [60, 50, 60]);
    let child_domain = bounds([-10, -20, -10], [70, 60, 70]);
    let mut parent_boxes = BTreeMap::from([
        ("root".into(), object("root", None, parent_domain, false)),
        (
            "auth".into(),
            object(
                "auth",
                Some("root"),
                bounds([32, 70, 32], [48, 80, 48]),
                true,
            ),
        ),
        ("app".into(), object("app", Some("root"), envelope, true)),
    ]);
    if block_vertical_passage {
        parent_boxes.insert(
            "unrelated".into(),
            object(
                "unrelated",
                Some("root"),
                bounds([29, 55, 29], [51, 65, 51]),
                true,
            ),
        );
    }
    let child_boxes = BTreeMap::from([
        ("app".into(), object("app", None, child_domain, false)),
        (
            "middleware".into(),
            object(
                "middleware",
                Some("app"),
                bounds([20, 0, 20], [40, 20, 40]),
                true,
            ),
        ),
    ]);
    let membership = vec!["auth-covers-middleware".to_owned()];
    let parent_request = FragmentRequest {
        id: "coverage-parent".into(),
        semantic_ids: membership.clone(),
        source: FragmentTerminal::Box("auth".into()),
        target: FragmentTerminal::Box("app".into()),
    };
    let parent_routes = route_fragments(
        &parent_boxes,
        parent_domain,
        std::slice::from_ref(&parent_request),
        2,
        &cancelled,
    )
    .unwrap();
    validate_fragments(
        &parent_boxes,
        parent_domain,
        &[parent_request],
        &parent_routes,
        2,
        &cancelled,
    )
    .unwrap();
    let parent_route = &parent_routes[0];
    let outer_attachment = *parent_route.points.last().unwrap();
    let before_attachment = parent_route.points[parent_route.points.len() - 2];
    let inward = Point::new(
        (outer_attachment.x - before_attachment.x).signum(),
        (outer_attachment.y - before_attachment.y).signum(),
        (outer_attachment.z - before_attachment.z).signum(),
    );
    let transform = DyadicTransform::new(child_domain.min, envelope.min, 1).unwrap();
    let inner_attachment = transform
        .inverse_boundary_terminal(outer_attachment, child_domain)
        .unwrap();
    let child_request = FragmentRequest {
        id: "coverage-child".into(),
        semantic_ids: membership.clone(),
        source: FragmentTerminal::Boundary {
            point: inner_attachment,
            inward,
        },
        target: FragmentTerminal::Box("middleware".into()),
    };
    let child_routes = route_fragments(
        &child_boxes,
        child_domain,
        std::slice::from_ref(&child_request),
        2,
        &cancelled,
    )
    .unwrap();
    validate_fragments(
        &child_boxes,
        child_domain,
        &[child_request],
        &child_routes,
        2,
        &cancelled,
    )
    .unwrap();
    let assembly = Assembly::new(
        vec![frame(parent_boxes), frame(child_boxes)],
        vec![
            AssemblyInstance::root("root"),
            AssemblyInstance::child("app", "root", transform),
        ],
    )
    .unwrap();
    let parent_map = assembly.transform_to("root", "root").unwrap();
    let child_map = assembly.transform_to("app", "root").unwrap();
    let presented_parent = parent_route
        .points
        .iter()
        .map(|p| parent_map.map_point(*p).unwrap())
        .collect::<Vec<_>>();
    let presented_child = child_routes[0]
        .points
        .iter()
        .map(|p| child_map.map_point(*p).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        presented_parent.last(),
        presented_child.first(),
        "no rounding or gap at boundary"
    );
    assert_eq!(parent_route.semantic_ids, child_routes[0].semantic_ids);
    assert_eq!(child_routes[0].semantic_ids, membership);
    assert_eq!(
        assembly.present("app", "root").unwrap().boxes["middleware"]
            .bounds
            .max
            .to_f64(),
        [45.0, 30.0, 45.0],
        "presentation geometry and route vertices must use the same transform",
    );
    if !block_vertical_passage {
        assert_eq!(parent_route.points.len(), 2);
        assert_eq!(child_routes[0].points.len(), 2);
        assert!(presented_parent.iter().chain(&presented_child).all(|p| {
            let xyz = p.to_f64();
            xyz[0] == 40.0 && xyz[2] == 40.0
        }));
        assert_eq!(presented_parent[0].to_f64(), [40.0, 70.0, 40.0]);
        assert_eq!(presented_child.last().unwrap().to_f64(), [40.0, 30.0, 40.0]);
    } else {
        assert!(
            parent_route.points.len() > 2,
            "unrelated box must force a detour"
        );
    }
}

#[test]
fn aligned_auth_route_joins_scaled_middleware_without_bends_or_rounding() {
    fixture(false);
}

#[test]
fn parent_obstacle_detour_still_joins_exact_child_boundary() {
    fixture(true);
}

#[test]
fn corner_parent_attachment_joins_the_selected_child_face_without_moving_or_bending() {
    let cancelled = AtomicBool::new(false);
    let parent_domain = bounds([0, 0, 0], [100, 100, 100]);
    let envelope = bounds([40, 0, 40], [60, 20, 60]);
    let child_domain = bounds([0, 0, 0], [80, 80, 80]);
    let parent_boxes = BTreeMap::from([
        ("root".into(), object("root", None, parent_domain, false)),
        (
            "auth".into(),
            object(
                "auth",
                Some("root"),
                bounds([20, 30, 20], [40, 50, 40]),
                true,
            ),
        ),
        ("app".into(), object("app", Some("root"), envelope, true)),
    ]);
    let child_boxes = BTreeMap::from([
        ("app".into(), object("app", None, child_domain, false)),
        (
            "middleware".into(),
            object(
                "middleware",
                Some("app"),
                bounds([0, 40, 0], [20, 60, 20]),
                true,
            ),
        ),
    ]);
    let membership = vec!["auth-covers-middleware".to_owned()];
    let request = FragmentRequest {
        id: "coverage-parent".into(),
        semantic_ids: membership.clone(),
        source: FragmentTerminal::Box("auth".into()),
        target: FragmentTerminal::Box("app".into()),
    };
    let parent = route_fragments(
        &parent_boxes,
        parent_domain,
        std::slice::from_ref(&request),
        2,
        &cancelled,
    )
    .unwrap();
    validate_fragments(
        &parent_boxes,
        parent_domain,
        std::slice::from_ref(&request),
        &parent,
        2,
        &cancelled,
    )
    .unwrap();
    assert_eq!(
        parent[0].points,
        [Point::new(40, 30, 40), Point::new(40, 20, 40)]
    );
    let ordinary = route_all(
        &parent_boxes,
        &[Edge {
            id: request.id,
            semantic_ids: membership.clone(),
            source: "auth".into(),
            target: "app".into(),
        }],
        2,
        &cancelled,
    )
    .unwrap();
    assert_eq!(
        ordinary[0].points, parent[0].points,
        "ordinary parent routing remains unchanged"
    );

    let outer_attachment = parent[0].points[1];
    let before_attachment = parent[0].points[0];
    let inward = Point::new(
        (outer_attachment.x - before_attachment.x).signum(),
        (outer_attachment.y - before_attachment.y).signum(),
        (outer_attachment.z - before_attachment.z).signum(),
    );
    let transform = DyadicTransform::new(child_domain.min, envelope.min, 2).unwrap();
    let inner_attachment = transform
        .inverse_boundary_terminal(outer_attachment, child_domain)
        .unwrap();
    assert_eq!(inner_attachment, Point::new(0, 80, 0));
    assert_eq!(inward, Point::new(0, -1, 0));
    let request = FragmentRequest {
        id: "coverage-child".into(),
        semantic_ids: membership.clone(),
        source: FragmentTerminal::Boundary {
            point: inner_attachment,
            inward,
        },
        target: FragmentTerminal::Box("middleware".into()),
    };
    let child = route_fragments(
        &child_boxes,
        child_domain,
        std::slice::from_ref(&request),
        2,
        &cancelled,
    )
    .unwrap();
    validate_fragments(
        &child_boxes,
        child_domain,
        &[request],
        &child,
        2,
        &cancelled,
    )
    .unwrap();
    assert_eq!(
        child[0].points,
        [Point::new(0, 80, 0), Point::new(0, 60, 0)]
    );
    assert_eq!(child[0].semantic_ids, parent[0].semantic_ids);
    let assembly = Assembly::new(
        vec![frame(parent_boxes), frame(child_boxes)],
        vec![
            AssemblyInstance::root("root"),
            AssemblyInstance::child("app", "root", transform),
        ],
    )
    .unwrap();
    let child_map = assembly.transform_to("app", "root").unwrap();
    assert_eq!(
        child_map
            .map_point(child[0].points[0])
            .unwrap()
            .to_integer()
            .unwrap(),
        outer_attachment
    );
    assert_eq!(
        child_map.map_point(child[0].points[1]).unwrap().to_f64(),
        [40.0, 15.0, 40.0]
    );
}
