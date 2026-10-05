use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Bounds, Edge, LAYOUT_VERSION, LayoutBox, LayoutError, LayoutSolution, Node, NodeKind, Point,
    Projection, QUALITY_VERSION, Route,
    assembly::{
        Assembly, AssemblyInstance, CanonicalFrame, DyadicTransform, MAX_TOTAL_ROUTE_POINTS,
    },
    assembly_plan::plan_frames,
    composed::{
        COMPOSITION_VERSION, CanonicalPins, ComposedMetrics, ComposedRoute, ComposedSolution,
        ComposedWork, FrameFragment, validate_composed,
    },
    routing::{FragmentRequest, FragmentTerminal, route_fragments},
};

fn p(x: i64, y: i64, z: i64) -> Point {
    Point::new(x, y, z)
}
fn b(min: Point, max: Point) -> Bounds {
    Bounds { min, max }
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
fn local(key: &str, boxes: BTreeMap<String, LayoutBox>) -> LayoutSolution {
    LayoutSolution {
        model_id: "independent-composition-validation".into(),
        revision: 1,
        projection_key: key.into(),
        version: LAYOUT_VERSION,
        boxes,
        routes: vec![],
        metrics: Default::default(),
        diagnostics: vec![],
    }
}
fn fixture(obstacle: bool) -> (Projection, ComposedSolution) {
    let domain = b(p(0, 0, 0), p(100, 100, 100));
    let host = b(p(20, 10, 20), p(60, 50, 60));
    let inside = b(p(-10, -20, -10), p(70, 60, 70));
    let mut top_boxes = BTreeMap::from([
        ("root".into(), object("root", None, domain, false)),
        (
            "auth".into(),
            object("auth", Some("root"), b(p(32, 70, 32), p(48, 80, 48)), true),
        ),
        ("app".into(), object("app", Some("root"), host, true)),
    ]);
    if obstacle {
        top_boxes.insert(
            "obstacle".into(),
            object(
                "obstacle",
                Some("root"),
                b(p(29, 55, 29), p(51, 65, 51)),
                true,
            ),
        );
    }
    let child_boxes = BTreeMap::from([
        ("app".into(), object("app", None, inside, false)),
        (
            "middleware".into(),
            object(
                "middleware",
                Some("app"),
                b(p(20, 0, 20), p(40, 20, 40)),
                true,
            ),
        ),
    ]);
    let mut nodes: Vec<_> = top_boxes
        .values()
        .map(|object| Node {
            id: object.id.clone(),
            semantic_ids: object.semantic_ids.clone(),
            parent: object.parent.clone(),
            kind: if object.id == "root" || object.id == "app" {
                NodeKind::Scope
            } else {
                NodeKind::Service
            },
            solid: object.id != "root" && object.id != "app",
            band: 0,
            min_size: p(1, 1, 1),
            max_size: p(1000, 1000, 1000),
            preferred_size: object.bounds.size(),
        })
        .collect();
    nodes.push(Node {
        id: "middleware".into(),
        semantic_ids: vec!["middleware".into()],
        parent: Some("app".into()),
        kind: NodeKind::Service,
        solid: true,
        band: 0,
        min_size: p(1, 1, 1),
        max_size: p(1000, 1000, 1000),
        preferred_size: p(20, 20, 20),
    });
    let edge = Edge {
        id: "coverage".into(),
        source: "auth".into(),
        target: "middleware".into(),
        semantic_ids: vec!["supplied:coverage".into()],
    };
    let projection = Projection {
        model_id: "independent-composition-validation".into(),
        revision: 1,
        key: "original-view".into(),
        root: "root".into(),
        nodes,
        edges: vec![edge.clone()],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let plan = plan_frames(&projection, &AtomicBool::new(false)).unwrap();
    let parent_request = FragmentRequest {
        id: edge.id.clone(),
        semantic_ids: edge.semantic_ids.clone(),
        source: FragmentTerminal::Box("auth".into()),
        target: FragmentTerminal::Box("app".into()),
    };
    let parent_route = route_fragments(
        &top_boxes,
        domain,
        std::slice::from_ref(&parent_request),
        2,
        &AtomicBool::new(false),
    )
    .unwrap()
    .remove(0);
    let attachment = *parent_route.points.last().unwrap();
    let previous = parent_route.points[parent_route.points.len() - 2];
    let normal = p(
        (attachment.x - previous.x).signum(),
        (attachment.y - previous.y).signum(),
        (attachment.z - previous.z).signum(),
    );
    let transform = DyadicTransform::new(inside.min, host.min, 1).unwrap();
    let child_request = FragmentRequest {
        id: edge.id.clone(),
        semantic_ids: edge.semantic_ids.clone(),
        source: FragmentTerminal::Boundary {
            point: transform
                .inverse_boundary_terminal(attachment, inside)
                .unwrap(),
            inward: normal,
        },
        target: FragmentTerminal::Box("middleware".into()),
    };
    let child_route = route_fragments(
        &child_boxes,
        inside,
        std::slice::from_ref(&child_request),
        2,
        &AtomicBool::new(false),
    )
    .unwrap()
    .remove(0);
    let assembly = Assembly::new(
        vec![
            CanonicalFrame::new(local(&plan.frames["root"].projection.key, top_boxes)).unwrap(),
            CanonicalFrame::new(local(&plan.frames["app"].projection.key, child_boxes)).unwrap(),
        ],
        vec![
            AssemblyInstance::root("root"),
            AssemblyInstance::child("app", "root", transform),
        ],
    )
    .unwrap();
    let solution = ComposedSolution {
        model_id: projection.model_id.clone(),
        revision: projection.revision,
        projection_key: projection.key.clone(),
        version: COMPOSITION_VERSION,
        quality_version: QUALITY_VERSION,
        clearance: 2,
        assembly,
        routes: BTreeMap::from([(
            "coverage".into(),
            ComposedRoute {
                edge,
                fragments: vec![
                    FrameFragment {
                        frame_id: "root".into(),
                        request: parent_request,
                        route: parent_route,
                    },
                    FrameFragment {
                        frame_id: "app".into(),
                        request: child_request,
                        route: child_route,
                    },
                ],
            },
        )]),
        metrics: ComposedMetrics::default(),
        // An allowance is reusable, not a reservation: sum allocations=4 while
        // actual evaluations=2. This is a valid shared two-evaluation budget.
        work: ComposedWork {
            budget: 2,
            allocated: BTreeMap::from([("root".into(), 2), ("app".into(), 2)]),
            evaluations: BTreeMap::from([("root".into(), 1), ("app".into(), 1)]),
            ..Default::default()
        },
    };
    (projection, solution)
}
fn valid(projection: &Projection, solution: &ComposedSolution) -> Result<(), LayoutError> {
    validate_composed(
        projection,
        solution,
        &CanonicalPins::new(),
        &AtomicBool::new(false),
    )
}

#[test]
fn feedback_uses_the_shared_budget_without_replacing_frame_evaluations() {
    let (projection, mut solution) = fixture(false);
    solution.work.budget = 5;
    solution.work.feedback_evaluations = 3;
    solution.work.feedback_improvements = 2;
    valid(&projection, &solution).unwrap();

    let mut too_much = solution.clone();
    too_much.work.feedback_evaluations = 4;
    assert!(valid(&projection, &too_much).is_err());
    let mut impossible_improvements = solution.clone();
    impossible_improvements.work.feedback_improvements = 4;
    assert!(valid(&projection, &impossible_improvements).is_err());
    let mut unevaluated_frame = solution.clone();
    unevaluated_frame.work.evaluations.insert("app".into(), 0);
    assert!(valid(&projection, &unevaluated_frame).is_err());
    // Feedback cannot certify that an individual frame reused accepted geometry.
    unevaluated_frame.work.reused.insert(
        "app".into(),
        visualyze_layout::composed::PlacementReuse::Previous,
    );
    valid(&projection, &unevaluated_frame).unwrap();

    let mut overflow = solution;
    overflow.work.budget = usize::MAX;
    overflow.work.feedback_evaluations = usize::MAX;
    overflow.work.feedback_improvements = 0;
    assert!(
        valid(&projection, &overflow)
            .unwrap_err()
            .to_string()
            .contains("overflow")
    );
}
fn rebuild(solution: &mut ComposedSolution, mut change: impl FnMut(&str, &mut LayoutSolution)) {
    let mut frames = Vec::new();
    let mut instances = Vec::new();
    for id in solution.assembly.frame_ids() {
        let frame = solution.assembly.frame(id).unwrap();
        let mut local = frame.solution().clone();
        change(id, &mut local);
        frames.push(CanonicalFrame::with_root(id, frame.bounds(), local).unwrap());
        instances.push(solution.assembly.instance(id).unwrap().clone());
    }
    solution.assembly = Assembly::new(frames, instances).unwrap();
}
fn rekey(projection: &Projection, solution: &mut ComposedSolution) {
    let plan = plan_frames(projection, &AtomicBool::new(false)).unwrap();
    rebuild(solution, |id, local| {
        local.projection_key = plan.frames[id].projection.key.clone()
    });
}

#[test]
fn validates_independently_constructed_exact_join_in_both_directions_and_with_obstacle() {
    for obstacle in [false, true] {
        let (mut projection, mut solution) = fixture(obstacle);
        valid(&projection, &solution).unwrap();
        if obstacle {
            assert!(solution.routes["coverage"].fragments[0].route.points.len() > 2);
        }
        let original = &mut projection.edges[0];
        std::mem::swap(&mut original.source, &mut original.target);
        let edge = projection.edges[0].clone();
        let route = solution.routes.get_mut("coverage").unwrap();
        route.edge = edge;
        route.fragments.reverse();
        for fragment in &mut route.fragments {
            fragment.route.points.reverse();
            std::mem::swap(&mut fragment.request.source, &mut fragment.request.target);
        }
        rekey(&projection, &mut solution);
        valid(&projection, &solution).unwrap();
    }
}

#[test]
fn surface_endpoint_stops_at_real_proxy_without_inventing_an_interior_fragment() {
    let (mut projection, mut solution) = fixture(false);
    projection.edges[0].target = "app".into();
    let route = solution.routes.get_mut("coverage").unwrap();
    route.edge = projection.edges[0].clone();
    route.fragments.pop();
    rekey(&projection, &mut solution);
    valid(&projection, &solution).unwrap();
    let extra = fixture(false).1.routes["coverage"].fragments[1].clone();
    solution
        .routes
        .get_mut("coverage")
        .unwrap()
        .fragments
        .push(extra);
    assert!(valid(&projection, &solution).is_err());
}

#[test]
fn rejects_missing_extra_reordered_and_redirected_original_relationships() {
    let (projection, solution) = fixture(false);
    for mutation in 0..7 {
        let mut changed = solution.clone();
        match mutation {
            0 => {
                changed.routes.clear();
            }
            1 => {
                changed
                    .routes
                    .insert("extra".into(), changed.routes["coverage"].clone());
            }
            2 => {
                changed.routes.get_mut("coverage").unwrap().fragments.pop();
            }
            3 => {
                changed
                    .routes
                    .get_mut("coverage")
                    .unwrap()
                    .fragments
                    .reverse();
            }
            4 => {
                changed.routes.get_mut("coverage").unwrap().edge.target = "app".into();
            }
            5 => {
                changed
                    .routes
                    .get_mut("coverage")
                    .unwrap()
                    .edge
                    .semantic_ids
                    .push("invented".into());
            }
            _ => {
                changed.routes.get_mut("coverage").unwrap().fragments[0].frame_id =
                    "unknown".into();
            }
        }
        assert!(valid(&projection, &changed).is_err(), "mutation {mutation}");
    }
}

#[test]
fn rejects_changed_terminals_gaps_normals_empty_segments_and_numeric_extremes() {
    let (projection, solution) = fixture(false);
    for mutation in 0..8 {
        let mut changed = solution.clone();
        let fragment = &mut changed.routes.get_mut("coverage").unwrap().fragments[1];
        match mutation {
            0 => fragment.request.target = FragmentTerminal::Box("app".into()),
            1 => {
                fragment.request.source = FragmentTerminal::Boundary {
                    point: p(30, 60, 30),
                    inward: p(0, 1, 0),
                }
            }
            2 => fragment.route.points[0].x += 1,
            3 => fragment.route.points.clear(),
            4 => fragment.route.points[0] = p(i64::MIN, i64::MAX, 0),
            5 => fragment.route.semantic_ids = vec!["different".into()],
            6 => fragment.route.id = "other-route".into(),
            _ => fragment.route.points[1] = fragment.route.points[0],
        }
        assert!(valid(&projection, &changed).is_err(), "mutation {mutation}");
    }
}

#[test]
fn rejects_obstacle_penetration_and_duplicate_paths_across_original_edges() {
    let (projection, mut solution) = fixture(true);
    solution.routes.get_mut("coverage").unwrap().fragments[0]
        .route
        .points = vec![p(40, 70, 40), p(40, 50, 40)];
    assert!(valid(&projection, &solution).is_err());
    let (mut projection, mut solution) = fixture(false);
    let mut repeated = solution.routes["coverage"].clone();
    repeated.edge.id = "parallel".into();
    repeated.edge.semantic_ids = vec!["supplied:parallel".into()];
    for fragment in &mut repeated.fragments {
        fragment.request.id = "parallel".into();
        fragment.route.id = "parallel".into();
        fragment.request.semantic_ids = repeated.edge.semantic_ids.clone();
        fragment.route.semantic_ids = repeated.edge.semantic_ids.clone();
    }
    projection.edges.push(repeated.edge.clone());
    solution.routes.insert("parallel".into(), repeated);
    rekey(&projection, &mut solution);
    assert!(
        matches!(valid(&projection,&solution),Err(LayoutError::Invalid(message)) if message.contains("identical connector"))
    );
}

#[test]
fn rejects_stale_identity_work_records_unknown_pins_and_duplicate_kernel_routes() {
    let (projection, solution) = fixture(false);
    for mutation in 0..10 {
        let mut changed = solution.clone();
        match mutation {
            0 => changed.revision += 1,
            1 => changed.version += 1,
            2 => changed.quality_version += 1,
            3 => changed.projection_key.push_str(":stale"),
            4 => changed.work.budget = 1,
            5 => {
                changed.work.allocated.remove("app");
            }
            6 => {
                changed.work.evaluations.insert("app".into(), 0);
            }
            7 => {
                changed.work.evaluations.insert("app".into(), 3);
            }
            8 => {
                changed.work.allocated.insert("root".into(), usize::MAX);
            }
            _ => {
                changed
                    .metrics
                    .frames
                    .insert("unknown".into(), Default::default());
            }
        }
        assert!(valid(&projection, &changed).is_err(), "mutation {mutation}");
    }
    for pins in [
        BTreeMap::from([("unknown".into(), BTreeMap::new())]),
        BTreeMap::from([(
            "app".into(),
            BTreeMap::from([("unknown".into(), Point::ZERO)]),
        )]),
        BTreeMap::from([(
            "app".into(),
            BTreeMap::from([("middleware".into(), p(i64::MAX, 0, 0))]),
        )]),
    ] {
        assert!(validate_composed(&projection, &solution, &pins, &AtomicBool::new(false)).is_err());
    }
    let mut changed = solution.clone();
    let fragment = &solution.routes["coverage"].fragments[0].route;
    rebuild(&mut changed, |id, local| {
        if id == "root" {
            local.routes.push(Route {
                edge_id: fragment.id.clone(),
                semantic_ids: fragment.semantic_ids.clone(),
                points: fragment.points.clone(),
                scope_ports: vec![],
            });
        }
    });
    assert!(valid(&projection, &changed).is_err());
    let mut changed = solution;
    rebuild(&mut changed, |_, local| {
        local.projection_key.push_str(":stale")
    });
    assert!(valid(&projection, &changed).is_err());
}

#[test]
fn cancellation_and_global_vertex_and_storage_limits_fail_before_route_processing() {
    let (projection, solution) = fixture(false);
    assert_eq!(
        validate_composed(
            &projection,
            &solution,
            &CanonicalPins::new(),
            &AtomicBool::new(true)
        ),
        Err(LayoutError::Cancelled)
    );
    let mut changed = solution.clone();
    changed.routes.get_mut("coverage").unwrap().fragments[0]
        .route
        .points = vec![Point::ZERO; MAX_TOTAL_ROUTE_POINTS + 1];
    assert!(
        matches!(valid(&projection,&changed),Err(LayoutError::Invalid(message)) if message.contains("vertices exceed budget"))
    );
    let mut changed = solution;
    changed.routes.get_mut("coverage").unwrap().fragments[0]
        .route
        .id = "x".repeat(8 * 1024 * 1024);
    assert!(
        matches!(valid(&projection,&changed),Err(LayoutError::Invalid(message)) if message.contains("identity storage"))
    );
}

#[test]
fn multiplied_membership_capacity_is_rejected_before_any_placement_allowance() {
    use visualyze_layout::{
        Algorithm, QualityProfile,
        composed::{ComposedRequest, solve_composed},
    };
    let (mut projection, _) = fixture(false);
    projection.edges = (0..600)
        .map(|index| Edge {
            id: format!("e:{index}"),
            source: "auth".into(),
            target: "middleware".into(),
            semantic_ids: (0..100)
                .map(|member| (index * 100 + member).to_string())
                .collect(),
        })
        .collect();
    // 60k original memberships fit the projection and each individual frame;
    // the retained edge/request/route copies across two frames exceed 262144.
    plan_frames(&projection, &AtomicBool::new(false)).unwrap();
    let result = solve_composed(ComposedRequest {
        projection: &projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &CanonicalPins::new(),
        work_budget: 0,
        cancelled: &AtomicBool::new(false),
    });
    assert!(
        matches!(result,Err(LayoutError::Invalid(message)) if message.contains("semantic membership exceeds budget"))
    );
}
