use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::AnchorPin,
    composed::{
        CanonicalPins, CanonicalPlacementCache, ComposedRequest, ComposedReuse, ComposedSolution,
        CompositionContext, solve_composed, solve_composed_cached, solve_composed_reorganized,
        validate_composed,
    },
    projection::LayoutPolicy,
};
use visualyze_model::provider::SemanticLens;

fn node(id: &str, parent: Option<&str>, solid: bool, band: i32) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        kind: if solid {
            NodeKind::Service
        } else {
            NodeKind::Group
        },
        band,
        solid,
        min_size: Point::new(12, 8, 12),
        preferred_size: Point::new(24, 16, 24),
        max_size: Point::new(48, 32, 48),
    }
}
fn fixture() -> (Projection, CanonicalPins, CompositionContext) {
    let p = Projection {
        key: "boundary-feedback".into(),
        model_id: "feedback-model".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false, 0),
            node("auth", Some("root"), true, 3),
            node("db", Some("root"), false, 0),
            node("inside", Some("db"), true, 0),
            node("far", Some("db"), true, 0),
        ],
        edges: vec![Edge {
            bundle: None,
            id: "auth-inside".into(),
            source: "auth".into(),
            target: "inside".into(),
            semantic_ids: vec!["coverage".into()],
        }],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let context = CompositionContext {
        model_id: p.model_id.clone(),
        root: p.root.clone(),
        lens: SemanticLens::Logical,
        policy: LayoutPolicy::Crud,
    };
    let pins = BTreeMap::from([(
        "db".into(),
        BTreeMap::from([("far".into(), Point::new(400, 0, 300))]),
    )]);
    (p, pins, context)
}
fn request<'a>(
    p: &'a Projection,
    pins: &'a CanonicalPins,
    cancel: &'a AtomicBool,
    feedback: usize,
) -> ComposedRequest<'a> {
    ComposedRequest {
        projection: p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile {
            feedback_candidates: feedback,
            ..Default::default()
        },
        pins,
        work_budget: 96,
        cancelled: cancel,
    }
}
fn assert_same_envelopes(a: &ComposedSolution, b: &ComposedSolution) {
    for id in a.assembly.frame_ids() {
        assert_eq!(
            a.assembly.frame(id).unwrap().bounds(),
            b.assembly.frame(id).unwrap().bounds(),
            "domain {id}"
        );
        assert_eq!(
            a.assembly.instance(id).unwrap().transform.scale_shift(),
            b.assembly.instance(id).unwrap().transform.scale_shift()
        );
        for (object, old) in &a.assembly.frame(id).unwrap().solution().boxes {
            let new = &b.assembly.frame(id).unwrap().solution().boxes[object];
            assert_eq!(old.bounds.size(), new.bounds.size());
            assert_eq!(old.bounds.center().y, new.bounds.center().y);
        }
    }
}
#[test]
fn boundary_feedback_shortens_actual_complete_routes_without_shrinking_or_relayering() {
    let (p, pins, _) = fixture();
    let cancel = AtomicBool::new(false);
    let baseline = solve_composed(request(&p, &pins, &cancel, 0)).unwrap();
    let improved = solve_composed(request(&p, &pins, &cancel, 4)).unwrap();
    println!(
        "baseline length={} bends={} cost={}, feedback length={} bends={} cost={} work={:?}",
        baseline.metrics.canonical_route_length,
        baseline.metrics.bends,
        baseline.metrics.structural_cost,
        improved.metrics.canonical_route_length,
        improved.metrics.bends,
        improved.metrics.structural_cost,
        improved.work
    );
    assert!(improved.work.feedback_improvements > 0);
    assert!(improved.metrics.canonical_route_length < baseline.metrics.canonical_route_length);
    assert!(improved.metrics.structural_cost < baseline.metrics.structural_cost);
    assert_same_envelopes(&baseline, &improved);
    validate_composed(&p, &improved, &pins, &cancel).unwrap();
    assert_eq!(
        improved.routes["auth-inside"].edge.semantic_ids,
        vec!["coverage"]
    );
    assert_eq!(improved.routes["auth-inside"].fragments.len(), 2);
    assert!(
        improved.work.evaluations.values().sum::<usize>() + improved.work.feedback_evaluations
            <= 96
    );
    let mut reversed = p.clone();
    reversed.nodes.reverse();
    reversed.edges.reverse();
    assert_eq!(
        improved,
        solve_composed(request(&reversed, &pins, &cancel, 4)).unwrap()
    );
}
#[test]
fn cache_navigation_and_repairs_freeze_accepted_interiors_but_reorganization_can_refine() {
    let (p, pins, context) = fixture();
    let cancel = AtomicBool::new(false);
    let mut cache = CanonicalPlacementCache::default();
    let old = solve_composed(request(&p, &pins, &cancel, 0)).unwrap();
    let repair = solve_composed_cached(
        request(&p, &pins, &cancel, 4),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &old)),
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    assert_eq!(repair.assembly, old.assembly);
    assert_eq!(repair.work.feedback_evaluations, 0);
    let cached = solve_composed_cached(
        request(&p, &pins, &cancel, 4),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    assert_eq!(cached.assembly, old.assembly);
    assert_eq!(cached.work.feedback_evaluations, 0);
    let proposed = solve_composed_reorganized(
        request(&p, &pins, &cancel, 4),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &old)),
            anchor_pins: &[],
        },
        &cache,
    )
    .unwrap();
    assert!(proposed.metrics.canonical_route_length < old.metrics.canonical_route_length);
    let unchanged = solve_composed_cached(
        request(&p, &pins, &cancel, 4),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    assert_eq!(
        unchanged.assembly, old.assembly,
        "unapproved refinement changed accepted cache"
    );
}
#[test]
fn pins_anchors_small_budgets_and_cancellation_remain_hard_constraints() {
    let (p, mut pins, context) = fixture();
    let cancel = AtomicBool::new(false);
    let old = solve_composed(request(&p, &pins, &cancel, 0)).unwrap();
    let pin = AnchorPin::capture(&old.assembly, "db", "root", "inside").unwrap();
    pins.get_mut("db")
        .unwrap()
        .insert("inside".into(), pin.local_point());
    let mut cache = CanonicalPlacementCache::default();
    let constrained = solve_composed_cached(
        request(&p, &pins, &cancel, 4),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &old)),
            anchor_pins: std::slice::from_ref(&pin),
        },
        &mut cache,
    )
    .unwrap();
    pin.validate(&constrained.assembly).unwrap();
    let budgeted = solve_composed(ComposedRequest {
        work_budget: 2,
        ..request(&p, &pins, &cancel, 4)
    })
    .unwrap();
    assert_eq!(budgeted.work.feedback_evaluations, 0);
    assert_eq!(budgeted.work.evaluations.values().sum::<usize>(), 2);
    let before = (cache.len(), cache.estimated_bytes());
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        solve_composed_cached(
            request(&p, &pins, &cancelled, 4),
            ComposedReuse {
                context: &context,
                previous: None,
                anchor_pins: &[]
            },
            &mut cache
        ),
        Err(visualyze_layout::LayoutError::Cancelled)
    ));
    assert_eq!(before, (cache.len(), cache.estimated_bytes()));
}

#[test]
fn moving_nested_proxy_reroutes_all_three_frames_and_preserves_exact_interior_geometry() {
    let (mut p, pins, _) = fixture();
    let cancel = AtomicBool::new(false);
    p.nodes.iter_mut().find(|n| n.id == "inside").unwrap().solid = false;
    p.nodes.push(node("leaf", Some("inside"), true, 0));
    p.edges[0].target = "leaf".into();
    let baseline = solve_composed(request(&p, &pins, &cancel, 0)).unwrap();
    let improved = solve_composed(request(&p, &pins, &cancel, 4)).unwrap();
    assert!(improved.work.feedback_improvements > 0);
    assert!(improved.metrics.canonical_route_length < baseline.metrics.canonical_route_length);
    assert_same_envelopes(&baseline, &improved);
    assert_eq!(
        baseline.assembly.frame("inside").unwrap().solution().boxes,
        improved.assembly.frame("inside").unwrap().solution().boxes
    );
    assert_ne!(
        baseline
            .assembly
            .instance("inside")
            .unwrap()
            .transform
            .translation(),
        improved
            .assembly
            .instance("inside")
            .unwrap()
            .transform
            .translation()
    );
    assert_eq!(improved.routes["auth-inside"].fragments.len(), 3);
    validate_composed(&p, &improved, &pins, &cancel).unwrap();
    let shown = improved.present(&cancel).unwrap();
    assert_eq!(shown.routes.len(), 1);
    assert_eq!(shown.routes[0].semantic_ids, vec!["coverage"]);
    assert_eq!(shown.routes[0].scope_ports.len(), 2);
}
