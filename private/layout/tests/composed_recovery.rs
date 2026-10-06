use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    composed::{
        CanonicalPins, CanonicalPlacementCache, ComposedRequest, ComposedReuse, CompositionContext,
        solve_composed, solve_composed_cached, validate_composed,
    },
    projection::LayoutPolicy,
};
use visualyze_model::provider::SemanticLens;

fn node(id: &str, parent: Option<&str>, solid: bool, size: Point) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        kind: if solid {
            NodeKind::Service
        } else {
            NodeKind::Group
        },
        band: 0,
        solid,
        min_size: size,
        preferred_size: size,
        max_size: size,
    }
}

fn fixture() -> (Projection, CanonicalPins) {
    let projection = Projection {
        key: "sealed-interior".into(),
        model_id: "recovery-model".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false, Point::new(100, 100, 100)),
            node("db", Some("root"), false, Point::new(100, 100, 100)),
            node("auth", Some("root"), true, Point::new(10, 10, 10)),
            node("a-blocker", Some("db"), true, Point::new(6, 28, 28)),
            node("target", Some("db"), true, Point::new(12, 12, 12)),
            node("right", Some("db"), true, Point::new(6, 28, 28)),
            node("bottom", Some("db"), true, Point::new(16, 6, 28)),
            node("top", Some("db"), true, Point::new(16, 6, 28)),
            node("front", Some("db"), true, Point::new(16, 16, 6)),
            node("back", Some("db"), true, Point::new(16, 16, 6)),
        ],
        edges: vec![Edge {
            bundle: None,
            id: "coverage".into(),
            source: "auth".into(),
            target: "target".into(),
            semantic_ids: vec!["claim:coverage".into()],
        }],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let pins = CanonicalPins::from([
        (
            "root".into(),
            BTreeMap::from([
                ("db".into(), Point::new(50, 50, 50)),
                ("auth".into(), Point::new(-25, 50, 50)),
            ]),
        ),
        (
            "db".into(),
            BTreeMap::from([
                ("target".into(), Point::new(20, 14, 14)),
                ("right".into(), Point::new(31, 14, 14)),
                ("bottom".into(), Point::new(20, 3, 14)),
                ("top".into(), Point::new(20, 25, 14)),
                ("front".into(), Point::new(20, 14, 3)),
                ("back".into(), Point::new(20, 14, 25)),
            ]),
        ),
    ]);
    (projection, pins)
}

fn request<'a>(
    projection: &'a Projection,
    pins: &'a CanonicalPins,
    feedback: usize,
    cancel: &'a AtomicBool,
) -> ComposedRequest<'a> {
    ComposedRequest {
        projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile {
            routed_candidates: 1,
            feedback_candidates: feedback,
            size_candidates: 0,
            ..Default::default()
        },
        pins,
        work_budget: 2 + feedback,
        cancelled: cancel,
    }
}

#[test]
fn public_solver_recovers_a_sealed_interior_after_independent_frame_placement() {
    let (mut projection, pins) = fixture();
    let cancel = AtomicBool::new(false);
    let original = solve_composed(request(&projection, &pins, 0, &cancel)).unwrap_err();
    assert!(
        matches!(original, LayoutError::Unroutable(_)),
        "{original:?}"
    );
    let recovered = solve_composed(request(&projection, &pins, 4, &cancel)).unwrap();
    validate_composed(&projection, &recovered, &pins, &cancel).unwrap();
    assert!(recovered.work.feedback_evaluations > 0);
    assert!(recovered.work.feedback_improvements > 0);
    assert_eq!(recovered.work.evaluations.values().sum::<usize>(), 2);
    assert!(recovered.work.feedback_evaluations <= 4);
    assert_eq!(recovered.routes.len(), 1);
    assert_eq!(recovered.routes["coverage"].edge, projection.edges[0]);
    assert_eq!(recovered.routes["coverage"].fragments.len(), 2);
    projection.nodes.reverse();
    projection.edges.reverse();
    assert_eq!(
        recovered,
        solve_composed(request(&projection, &pins, 4, &cancel)).unwrap()
    );
}

#[test]
fn failed_recovery_preserves_the_accepted_cache_and_revision_watermark() {
    let (mut projection, mut pins) = fixture();
    let cancel = AtomicBool::new(false);
    let context = CompositionContext {
        model_id: projection.model_id.clone(),
        root: projection.root.clone(),
        lens: SemanticLens::Logical,
        policy: LayoutPolicy::Crud,
    };
    let reuse = ComposedReuse {
        context: &context,
        previous: None,
        anchor_pins: &[],
    };
    let mut cache = CanonicalPlacementCache::default();
    let accepted =
        solve_composed_cached(request(&projection, &pins, 4, &cancel), reuse, &mut cache).unwrap();
    let before = (cache.len(), cache.estimated_bytes());
    projection.revision = 2;
    // Fix the only movable blocker back against the target's left face. The
    // newer revision cannot publish this sealed state or advance the watermark.
    pins.get_mut("db")
        .unwrap()
        .insert("a-blocker".into(), Point::new(9, 14, 14));
    let failed = solve_composed_cached(request(&projection, &pins, 4, &cancel), reuse, &mut cache);
    assert!(
        matches!(failed, Err(LayoutError::Unroutable(_))),
        "{failed:?}"
    );
    assert_eq!(before, (cache.len(), cache.estimated_bytes()));
    projection.revision = 1;
    pins.get_mut("db").unwrap().remove("a-blocker");
    let restored =
        solve_composed_cached(request(&projection, &pins, 4, &cancel), reuse, &mut cache).unwrap();
    assert_eq!(restored.assembly, accepted.assembly);
    assert_eq!(restored.routes, accepted.routes);
    assert_eq!(restored.work.feedback_evaluations, 0);
    assert_eq!(restored.work.evaluations.values().sum::<usize>(), 0);
}
