use super::*;
use crate::{LAYOUT_VERSION, LayoutBox, Node, NodeKind};

fn bounds(min: [i64; 3], max: [i64; 3]) -> Bounds {
    Bounds {
        min: Point::new(min[0], min[1], min[2]),
        max: Point::new(max[0], max[1], max[2]),
    }
}

fn node(id: &str, parent: Option<&str>, solid: bool, band: i32, size: Point) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![format!("semantic:{id}")],
        parent: parent.map(str::to_owned),
        kind: if solid {
            NodeKind::Service
        } else {
            NodeKind::Group
        },
        band,
        solid,
        min_size: size,
        preferred_size: size,
        max_size: size,
    }
}

fn fixture() -> (Projection, CanonicalPins) {
    let projection = Projection {
        key: "blocked-child-boundary".into(),
        model_id: "composed-recovery".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false, 0, Point::new(120, 150, 120)),
            node("db", Some("root"), false, 0, Point::new(100, 100, 100)),
            node("auth", Some("root"), true, 1, Point::new(10, 10, 10)),
            node("inside", Some("db"), true, 0, Point::new(10, 10, 10)),
            node("blocker", Some("db"), true, 0, Point::new(20, 4, 20)),
        ],
        edges: vec![Edge {
            bundle: None,
            id: "auth-inside".into(),
            source: "auth".into(),
            target: "inside".into(),
            semantic_ids: vec!["coverage:inside".into()],
        }],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let pins = BTreeMap::from([
        (
            "root".into(),
            BTreeMap::from([
                ("auth".into(), Point::new(50, 125, 50)),
                ("db".into(), Point::new(50, 50, 50)),
            ]),
        ),
        (
            "db".into(),
            BTreeMap::from([("inside".into(), Point::new(50, 45, 50))]),
        ),
    ]);
    (projection, pins)
}

/// This is a valid already-placed assembly, with deliberately fixed interiors.
/// The parent route fixes the child's ingress to (50, 100, 50). Its two-unit
/// normal escape penetrates the unrelated blocker's clearance. Moving only
/// that blocker twelve units horizontally clears the exact same ingress.
fn seed(projection: &Projection) -> (FramePlan, Vec<String>, Assembly) {
    let cancel = AtomicBool::new(false);
    let plan = plan_frames(projection, &cancel).unwrap();
    let order = frame_order(&plan).unwrap();
    let geometry = BTreeMap::from([
        ("root", bounds([-10, -10, -10], [110, 140, 110])),
        ("db", bounds([0, 0, 0], [100, 100, 100])),
        ("auth", bounds([45, 120, 45], [55, 130, 55])),
        ("inside", bounds([45, 40, 45], [55, 50, 55])),
        ("blocker", bounds([40, 94, 40], [60, 98, 60])),
        ("blocker-left", bounds([40, 94, 40], [50, 98, 60])),
        ("blocker-right", bounds([50, 94, 40], [60, 98, 60])),
        ("wall", bounds([26, 94, 40], [34, 98, 60])),
    ]);
    let frames = plan
        .frames
        .iter()
        .map(|(id, frame)| {
            let local = LayoutSolution {
                model_id: projection.model_id.clone(),
                revision: projection.revision,
                projection_key: frame.projection.key.clone(),
                version: LAYOUT_VERSION,
                boxes: frame
                    .projection
                    .nodes
                    .iter()
                    .map(|node| {
                        (
                            node.id.clone(),
                            LayoutBox {
                                id: node.id.clone(),
                                semantic_ids: node.semantic_ids.clone(),
                                parent: node.parent.clone(),
                                bounds: geometry[node.id.as_str()],
                                solid: node.solid,
                            },
                        )
                    })
                    .collect(),
                routes: vec![],
                metrics: Metrics::default(),
                diagnostics: vec![],
            };
            crate::validation::validate_boxes(&frame.projection, &local, &BTreeMap::new()).unwrap();
            CanonicalFrame::with_root(id, geometry[id.as_str()], local).unwrap()
        })
        .collect();
    let assembly = Assembly::new(
        frames,
        vec![
            AssemblyInstance::root("root"),
            AssemblyInstance::child("db", "root", DyadicTransform::identity()),
        ],
    )
    .unwrap();
    (plan, order, assembly)
}

fn attempt(
    projection: &Projection,
    pins: &CanonicalPins,
    eligible: BTreeSet<String>,
    allowance: usize,
    cancelled: &AtomicBool,
) -> (
    Result<ComposedSolution, LayoutError>,
    ComposedWork,
    Assembly,
) {
    let (plan, order, assembly) = seed(projection);
    let mut work = ComposedWork {
        budget: 2 + allowance,
        allocated: BTreeMap::from([("root".into(), 1), ("db".into(), 1)]),
        evaluations: BTreeMap::from([("root".into(), 1), ("db".into(), 1)]),
        ..Default::default()
    };
    let request = ComposedRequest {
        projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile {
            feedback_candidates: allowance,
            ..Default::default()
        },
        pins,
        work_budget: work.budget,
        cancelled,
    };
    let result = route_or_recover(
        request,
        &plan,
        &order,
        assembly.clone(),
        &mut work,
        &eligible,
        allowance,
        None,
    );
    (result, work, assembly)
}

fn child_eligible() -> BTreeSet<String> {
    BTreeSet::from(["db".into()])
}

#[test]
fn fixed_child_boundary_failure_recovers_with_one_horizontal_blocker_move() {
    let (projection, pins) = fixture();
    let cancel = AtomicBool::new(false);
    let (initial, initial_work, before) = attempt(&projection, &pins, child_eligible(), 0, &cancel);
    assert!(matches!(initial, Err(LayoutError::Unroutable(_))));
    assert_eq!(initial_work.feedback_evaluations, 0);
    assert_eq!(initial_work.cache.routing_misses, 2);

    let (result, work, _) = attempt(&projection, &pins, child_eligible(), 4, &cancel);
    let recovered = result.unwrap();
    assert_eq!(work.feedback_evaluations, 1);
    assert_eq!(work.feedback_improvements, 1);
    assert!(work.evaluations.values().sum::<usize>() + work.feedback_evaluations <= work.budget);
    validate_composed(&projection, &recovered, &pins, &cancel).unwrap();
    assert_eq!(recovered.routes.len(), 1);
    let route = &recovered.routes["auth-inside"];
    assert_eq!(route.edge, projection.edges[0]);
    assert_eq!(route.fragments.len(), 2);
    assert_eq!(route.fragments[0].frame_id, "root");
    assert_eq!(route.fragments[1].frame_id, "db");
    assert_eq!(
        route.fragments[1].request.source,
        FragmentTerminal::Boundary {
            point: Point::new(50, 100, 50),
            inward: Point::new(0, -1, 0),
        }
    );
    for fragment in &route.fragments {
        assert_eq!(fragment.route.id, "auth-inside");
        assert_eq!(fragment.route.semantic_ids, vec!["coverage:inside"]);
    }
    let parent_join = *route.fragments[0].route.points.last().unwrap();
    let child_join = route.fragments[1].route.points[0];
    assert_eq!(
        recovered
            .assembly
            .instance("db")
            .unwrap()
            .transform
            .map_point(child_join)
            .unwrap()
            .to_integer()
            .unwrap(),
        parent_join
    );
    for id in before.frame_ids() {
        let old = before.frame(id).unwrap();
        let new = recovered.assembly.frame(id).unwrap();
        assert_eq!(old.bounds(), new.bounds());
        assert_eq!(before.instance(id), recovered.assembly.instance(id));
        for (object, old_box) in &old.solution().boxes {
            let new_box = &new.solution().boxes[object];
            assert_eq!(old_box.bounds.size(), new_box.bounds.size());
            assert_eq!(old_box.bounds.center().y, new_box.bounds.center().y);
            if object == "blocker" {
                assert_ne!(old_box.bounds, new_box.bounds);
            } else {
                assert_eq!(old_box, new_box);
            }
        }
    }
    let (bounded, bounded_work, _) = attempt(&projection, &pins, child_eligible(), 1, &cancel);
    let bounded = bounded.unwrap();
    assert_eq!(bounded_work.feedback_evaluations, 1);
    assert_eq!(bounded_work.budget, 3);
    assert_eq!(bounded.assembly, recovered.assembly);
    assert_eq!(bounded.routes, recovered.routes);
}

#[test]
fn pinned_blockers_and_ineligible_existing_interiors_cannot_be_repaired() {
    let (projection, mut pins) = fixture();
    let cancel = AtomicBool::new(false);
    let (initial, _, _) = attempt(&projection, &pins, child_eligible(), 0, &cancel);
    let error = initial.unwrap_err();
    pins.get_mut("db")
        .unwrap()
        .insert("blocker".into(), Point::new(50, 96, 50));
    let (pinned, work, _) = attempt(&projection, &pins, child_eligible(), 4, &cancel);
    assert_eq!(pinned.unwrap_err(), error);
    assert_eq!(work.feedback_evaluations, 0);
    pins.get_mut("db").unwrap().remove("blocker");
    let (frozen, work, _) = attempt(
        &projection,
        &pins,
        BTreeSet::from(["root".into()]),
        4,
        &cancel,
    );
    assert_eq!(frozen.unwrap_err(), error);
    assert_eq!(work.feedback_evaluations, 0);
    assert_eq!(work.feedback_improvements, 0);
}

#[test]
fn recovery_is_input_order_independent_and_honors_cancellation_before_trials() {
    let (mut projection, pins) = fixture();
    let cancel = AtomicBool::new(false);
    let (result, work, _) = attempt(&projection, &pins, child_eligible(), 4, &cancel);
    projection.nodes.reverse();
    projection.edges.reverse();
    let (reordered, reordered_work, _) = attempt(&projection, &pins, child_eligible(), 4, &cancel);
    assert_eq!(result.unwrap(), reordered.unwrap());
    assert_eq!(work, reordered_work);

    let cancelled = AtomicBool::new(true);
    let (result, work, _) = attempt(&projection, &pins, child_eligible(), 4, &cancelled);
    assert_eq!(result, Err(LayoutError::Cancelled));
    assert_eq!(work.feedback_evaluations, 0);
    assert_eq!(work.feedback_improvements, 0);
    assert_eq!(work.cache.routing_hits + work.cache.routing_misses, 0);
}

#[test]
fn recovery_can_advance_through_still_blocked_geometry_without_publishing_it() {
    let (mut projection, pins) = fixture();
    projection.nodes.retain(|node| node.id != "blocker");
    for id in ["blocker-left", "blocker-right"] {
        projection
            .nodes
            .push(node(id, Some("db"), true, 0, Point::new(10, 4, 20)));
    }
    let cancel = AtomicBool::new(false);
    let (initial, _, _) = attempt(&projection, &pins, child_eligible(), 0, &cancel);
    let initial_error = initial.unwrap_err();
    assert!(matches!(initial_error, LayoutError::Unroutable(_)));
    let (incomplete, work, _) = attempt(&projection, &pins, child_eligible(), 1, &cancel);
    assert_eq!(incomplete.unwrap_err(), initial_error);
    assert_eq!(work.feedback_evaluations, 1);
    assert_eq!(work.feedback_improvements, 0);
    assert_eq!(
        work.evaluations.values().sum::<usize>() + work.feedback_evaluations,
        3
    );

    let (complete, work, before) = attempt(&projection, &pins, child_eligible(), 2, &cancel);
    let complete = complete.unwrap();
    assert_eq!(work.feedback_evaluations, 2);
    assert_eq!(work.feedback_improvements, 1);
    assert_eq!(
        work.evaluations.values().sum::<usize>() + work.feedback_evaluations,
        4
    );
    assert_eq!(work.budget, 4);
    validate_composed(&projection, &complete, &pins, &cancel).unwrap();
    let old = before.frame("db").unwrap();
    let new = complete.assembly.frame("db").unwrap();
    assert_eq!(old.bounds(), new.bounds());
    assert_eq!(before.instance("db"), complete.assembly.instance("db"));
    for (id, shift) in [
        ("blocker-left", Point::new(-2, 0, 0)),
        ("blocker-right", Point::new(2, 0, 0)),
    ] {
        assert_eq!(
            new.solution().boxes[id].bounds,
            old.solution().boxes[id].bounds.translated(shift)
        );
    }
    assert_eq!(complete.routes.len(), 1);
    assert_eq!(complete.routes["auth-inside"].fragments.len(), 2);
}

#[test]
fn rejected_overlap_candidate_consumes_the_same_shared_recovery_allowance() {
    let (mut projection, mut pins) = fixture();
    projection
        .nodes
        .push(node("wall", Some("db"), true, 0, Point::new(8, 4, 20)));
    pins.get_mut("db")
        .unwrap()
        .insert("wall".into(), Point::new(30, 96, 50));
    let cancel = AtomicBool::new(false);
    // The shortest stable first proposal moves the blocker left by twelve;
    // this overlaps the pinned wall and must be rejected and charged.
    let (rejected, work, _) = attempt(&projection, &pins, child_eligible(), 1, &cancel);
    assert!(matches!(rejected, Err(LayoutError::Unroutable(_))));
    assert_eq!(work.feedback_evaluations, 1);
    assert_eq!(work.feedback_improvements, 0);
    assert_eq!(
        work.evaluations.values().sum::<usize>() + work.feedback_evaluations,
        work.budget
    );
    let (complete, work, before) = attempt(&projection, &pins, child_eligible(), 2, &cancel);
    let complete = complete.unwrap();
    assert_eq!(work.feedback_evaluations, 2);
    assert_eq!(work.feedback_improvements, 1);
    assert_eq!(
        work.evaluations.values().sum::<usize>() + work.feedback_evaluations,
        work.budget
    );
    validate_composed(&projection, &complete, &pins, &cancel).unwrap();
    assert_eq!(
        before.frame("db").unwrap().solution().boxes["wall"],
        complete.assembly.frame("db").unwrap().solution().boxes["wall"]
    );
}
