use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::AnchorPin,
    composed::{
        CanonicalPins, CanonicalPlacementCache, ComposedRequest, ComposedReuse, ComposedSolution,
        CompositionContext, cache_accepted_composed, solve_composed_cached,
        solve_composed_reorganized, validate_composed,
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
fn fixture() -> (Projection, CompositionContext) {
    let p = Projection {
        key: "reorganize-nested".into(),
        model_id: "reorganize-model".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false, 0),
            node("auth", Some("root"), true, 3),
            node("db", Some("root"), false, 0),
            node("table", Some("db"), false, 0),
            node("row", Some("table"), true, 0),
        ],
        edges: vec![Edge {
            bundle: None,
            id: "auth-row".into(),
            source: "auth".into(),
            target: "row".into(),
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
    (p, context)
}
fn request<'a>(
    p: &'a Projection,
    pins: &'a CanonicalPins,
    cancel: &'a AtomicBool,
) -> ComposedRequest<'a> {
    ComposedRequest {
        projection: p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins,
        work_budget: 192,
        cancelled: cancel,
    }
}
fn accepted(
    p: &Projection,
    context: &CompositionContext,
    cache: &mut CanonicalPlacementCache,
) -> ComposedSolution {
    solve_composed_cached(
        request(p, &CanonicalPins::new(), &AtomicBool::new(false)),
        ComposedReuse {
            context,
            previous: None,
            anchor_pins: &[],
        },
        cache,
    )
    .unwrap()
}

fn entered_table(p: &Projection, context: &CompositionContext) -> (Projection, CompositionContext) {
    let mut p = p.clone();
    p.root = "table".into();
    p.key = "entered-table".into();
    p.nodes
        .retain(|node| node.id == "table" || node.parent.as_deref() == Some("table"));
    p.nodes
        .iter_mut()
        .find(|node| node.id == "table")
        .unwrap()
        .parent = None;
    p.edges.clear();
    let mut context = context.clone();
    context.root = p.root.clone();
    (p, context)
}

#[test]
fn explicit_acceptance_publishes_new_interiors_and_undo_republishes_the_previous_arrangement() {
    let (p, context) = fixture();
    let cancel = AtomicBool::new(false);
    let pins = CanonicalPins::new();
    let mut cache = CanonicalPlacementCache::default();
    let temporary_pin = BTreeMap::from([(
        "table".into(),
        BTreeMap::from([("row".into(), Point::new(600, 0, 0))]),
    )]);
    let pinned = solve_composed_cached(
        request(&p, &temporary_pin, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    // Removing the temporary pin preserves the established off-center layout
    // and caches it under the same unconstrained inputs used by reorganization.
    let old = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &pinned)),
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    let candidate = solve_composed_reorganized(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &old)),
            anchor_pins: &[],
        },
        &cache,
    )
    .unwrap();
    let old_row = old.assembly.frame("table").unwrap().solution().boxes["row"].clone();
    let new_row = candidate.assembly.frame("table").unwrap().solution().boxes["row"].clone();
    assert_ne!(old_row, new_row);
    let (entered, entered_context) = entered_table(&p, &context);
    let before_apply = accepted(&entered, &entered_context, &mut cache.clone());
    assert_eq!(before_apply.work.cache.hits, 1);
    assert_eq!(
        before_apply
            .assembly
            .frame("table")
            .unwrap()
            .solution()
            .boxes["row"],
        old_row
    );

    let original_candidate = candidate.clone();
    let stats = cache_accepted_composed(
        request(&p, &pins, &cancel),
        &context,
        &candidate,
        &mut cache,
    )
    .unwrap();
    assert_eq!(stats.entries, cache.len());
    assert_eq!(stats.hits + stats.previous_hits + stats.misses, 0);
    assert_eq!(candidate, original_candidate);
    let after_apply = accepted(&entered, &entered_context, &mut cache.clone());
    assert_eq!(after_apply.work.cache.hits, 1);
    assert_eq!(after_apply.work.evaluations["table"], 0);
    assert_eq!(
        after_apply
            .assembly
            .frame("table")
            .unwrap()
            .solution()
            .boxes["row"],
        new_row
    );

    cache_accepted_composed(request(&p, &pins, &cancel), &context, &old, &mut cache).unwrap();
    let after_undo = accepted(&entered, &entered_context, &mut cache);
    assert_eq!(after_undo.work.cache.hits, 1);
    assert_eq!(
        after_undo.assembly.frame("table").unwrap().solution().boxes["row"],
        old_row
    );
}

#[test]
fn invalid_or_cancelled_accepted_cache_publication_is_transactional() {
    let (mut p, context) = fixture();
    let cancel = AtomicBool::new(false);
    let pins = CanonicalPins::new();
    let mut cache = CanonicalPlacementCache::default();
    let old = accepted(&p, &context, &mut cache);
    let original = (cache.len(), cache.estimated_bytes());
    let req = request(&p, &pins, &cancel);
    let mut wrong_context = context.clone();
    wrong_context.root = "elsewhere".into();
    assert!(cache_accepted_composed(req, &wrong_context, &old, &mut cache).is_err());
    assert!(
        cache_accepted_composed(
            ComposedRequest {
                work_budget: 191,
                ..req
            },
            &context,
            &old,
            &mut cache
        )
        .is_err()
    );
    assert!(
        cache_accepted_composed(
            ComposedRequest {
                quality: QualityProfile {
                    clearance: 3,
                    ..req.quality
                },
                ..req
            },
            &context,
            &old,
            &mut cache
        )
        .is_err()
    );
    let conflicting = BTreeMap::from([(
        "table".into(),
        BTreeMap::from([("row".into(), Point::new(600, 0, 0))]),
    )]);
    assert!(
        cache_accepted_composed(
            ComposedRequest {
                pins: &conflicting,
                ..req
            },
            &context,
            &old,
            &mut cache
        )
        .is_err()
    );
    assert_eq!(
        cache_accepted_composed(
            ComposedRequest {
                cancelled: &AtomicBool::new(true),
                ..req
            },
            &context,
            &old,
            &mut cache
        ),
        Err(LayoutError::Cancelled)
    );
    assert_eq!((cache.len(), cache.estimated_bytes()), original);
    assert_eq!(accepted(&p, &context, &mut cache).assembly, old.assembly);

    let old_projection = p.clone();
    p.revision += 1;
    p.key = "new-revision".into();
    let newer = accepted(&p, &context, &mut cache);
    let before_stale = (cache.len(), cache.estimated_bytes());
    assert!(
        cache_accepted_composed(
            request(&old_projection, &pins, &cancel),
            &context,
            &old,
            &mut cache
        )
        .is_err()
    );
    assert_eq!((cache.len(), cache.estimated_bytes()), before_stale);
    assert_eq!(accepted(&p, &context, &mut cache).assembly, newer.assembly);
}

#[test]
fn explicit_reorganization_searches_free_frames_instead_of_reusing_accepted_placements() {
    let (p, context) = fixture();
    let mut cache = CanonicalPlacementCache::default();
    let previous = accepted(&p, &context, &mut cache);
    let cache_before = (cache.len(), cache.estimated_bytes());
    let result = solve_composed_reorganized(
        request(&p, &CanonicalPins::new(), &AtomicBool::new(false)),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &previous)),
            anchor_pins: &[],
        },
        &cache,
    )
    .unwrap();
    assert!(result.work.reused.is_empty());
    assert!(result.work.evaluations.values().all(|n| *n > 0));
    assert!(
        result.work.evaluations.values().sum::<usize>() + result.work.feedback_evaluations <= 192
    );
    assert_eq!(result.work.cache.hits, 0);
    assert_eq!(result.work.cache.previous_hits, 0);
    assert_eq!((cache.len(), cache.estimated_bytes()), cache_before);
    // The search does not replace previously accepted interior cache entries.
    let hit = accepted(&p, &context, &mut cache);
    assert_eq!(hit.work.cache.hits, 3);
    assert_eq!(hit.assembly, previous.assembly);
}

#[test]
fn a_root_local_anchor_and_canonical_pin_allow_real_search_around_them() {
    let (mut p, context) = fixture();
    p.nodes
        .extend((0..7).map(|i| node(&format!("service-{i}"), Some("root"), true, 1)));
    let mut cache = CanonicalPlacementCache::default();
    let previous = accepted(&p, &context, &mut cache);
    let before = previous.clone();
    let pin = AnchorPin::capture(&previous.assembly, "root", "root", "auth").unwrap();
    let canonical = BTreeMap::from([(
        "root".into(),
        BTreeMap::from([(
            "db".into(),
            previous.assembly.frame("root").unwrap().solution().boxes["db"]
                .bounds
                .center(),
        )]),
    )]);
    let result = solve_composed_reorganized(
        request(&p, &canonical, &AtomicBool::new(false)),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &previous)),
            anchor_pins: std::slice::from_ref(&pin),
        },
        &cache,
    )
    .unwrap();
    assert!(result.work.evaluations["root"] > 1);
    assert!(!result.work.reused.contains_key("root"));
    pin.validate(&result.assembly).unwrap();
    validate_composed(&p, &result, &canonical, &AtomicBool::new(false)).unwrap();
    assert_eq!(previous, before);
}

#[test]
fn nested_anchor_locks_hosting_frames_but_searches_its_owner_without_relaxing_a_non_improvement() {
    let (p, context) = fixture();
    let mut cache = CanonicalPlacementCache::default();
    let previous = accepted(&p, &context, &mut cache);
    let pin = AnchorPin::capture(&previous.assembly, "table", "root", "row").unwrap();
    let before = previous.clone();
    let cache_before = (cache.len(), cache.estimated_bytes());
    let result = solve_composed_reorganized(
        request(&p, &CanonicalPins::new(), &AtomicBool::new(false)),
        ComposedReuse {
            context: &context,
            previous: Some((&context, &previous)),
            anchor_pins: std::slice::from_ref(&pin),
        },
        &cache,
    )
    .unwrap();
    assert_eq!(result.work.evaluations["root"], 0);
    assert_eq!(result.work.evaluations["db"], 0);
    assert!(result.work.evaluations["table"] > 0);
    assert_eq!(
        result.metrics.structural_cost,
        previous.metrics.structural_cost
    );
    pin.validate(&result.assembly).unwrap();
    for id in ["root", "db", "table"] {
        assert_eq!(
            previous.assembly.frame(id).unwrap().bounds(),
            result.assembly.frame(id).unwrap().bounds()
        );
        assert_eq!(
            previous.assembly.instance(id).unwrap(),
            result.assembly.instance(id).unwrap()
        );
    }
    assert_eq!(previous, before);
    assert_eq!((cache.len(), cache.estimated_bytes()), cache_before);
}

#[test]
fn search_budget_counts_only_free_frame_evaluations_and_cannot_round_zero_up() {
    let (p, context) = fixture();
    let mut cache = CanonicalPlacementCache::default();
    let previous = accepted(&p, &context, &mut cache);
    let pin = AnchorPin::capture(&previous.assembly, "table", "root", "row").unwrap();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let reuse = ComposedReuse {
        context: &context,
        previous: Some((&context, &previous)),
        anchor_pins: std::slice::from_ref(&pin),
    };
    let result = solve_composed_reorganized(
        ComposedRequest {
            work_budget: 1,
            ..request(&p, &pins, &cancel)
        },
        reuse,
        &cache,
    )
    .unwrap();
    assert_eq!(result.work.evaluations.values().sum::<usize>(), 1);
    pin.validate(&result.assembly).unwrap();
    assert!(
        solve_composed_reorganized(
            ComposedRequest {
                work_budget: 0,
                ..request(&p, &pins, &cancel)
            },
            reuse,
            &cache
        )
        .is_err()
    );
}

#[test]
fn stale_search_cancel_and_conflicting_pin_never_change_accepted_geometry_or_cache() {
    let (mut p, context) = fixture();
    let mut cache = CanonicalPlacementCache::default();
    let previous = accepted(&p, &context, &mut cache);
    let before = previous.clone();
    let cache_before = (cache.len(), cache.estimated_bytes());
    let pin = AnchorPin::capture(&previous.assembly, "table", "root", "row").unwrap();
    let reuse = ComposedReuse {
        context: &context,
        previous: Some((&context, &previous)),
        anchor_pins: std::slice::from_ref(&pin),
    };
    assert_eq!(
        solve_composed_reorganized(
            request(&p, &CanonicalPins::new(), &AtomicBool::new(true)),
            reuse,
            &cache
        ),
        Err(LayoutError::Cancelled)
    );
    let conflict = BTreeMap::from([(
        "table".into(),
        BTreeMap::from([("row".into(), pin.local_point() + Point::new(1, 0, 0))]),
    )]);
    assert!(
        solve_composed_reorganized(
            request(&p, &conflict, &AtomicBool::new(false)),
            reuse,
            &cache
        )
        .is_err()
    );
    p.revision += 1;
    assert!(
        solve_composed_reorganized(
            request(&p, &CanonicalPins::new(), &AtomicBool::new(false)),
            reuse,
            &cache
        )
        .unwrap_err()
        .to_string()
        .contains("accepted model revision")
    );
    assert_eq!(previous, before);
    assert_eq!((cache.len(), cache.estimated_bytes()), cache_before);
}
