use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::AnchorPin,
    composed::{
        CanonicalCacheLimits, CanonicalPins, CanonicalPlacementCache, ComposedRequest,
        ComposedReuse, ComposedSolution, CompositionContext, solve_composed_cached,
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
fn fixture() -> Projection {
    Projection {
        key: "cached-nested".into(),
        model_id: "cached-model".into(),
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
    }
}
fn context(p: &Projection) -> CompositionContext {
    CompositionContext {
        model_id: p.model_id.clone(),
        root: p.root.clone(),
        lens: SemanticLens::Logical,
        policy: LayoutPolicy::Crud,
    }
}
fn run(
    p: &Projection,
    ctx: &CompositionContext,
    previous: Option<(&CompositionContext, &ComposedSolution)>,
    anchors: &[AnchorPin],
    cache: &mut CanonicalPlacementCache,
) -> Result<ComposedSolution, LayoutError> {
    solve_composed_cached(
        ComposedRequest {
            projection: p,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins: &CanonicalPins::new(),
            work_budget: 192,
            cancelled: &AtomicBool::new(false),
        },
        ComposedReuse {
            context: ctx,
            previous,
            anchor_pins: anchors,
        },
        cache,
    )
}
#[test]
fn exact_placement_hits_spend_no_candidates_and_always_return_complete_valid_routes() {
    let p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(first.work.cache.misses, 3);
    assert_eq!(first.work.cache.routing_misses, 3);
    assert_eq!(first.work.cache.routing_hits, 0);
    assert_eq!(cache.len(), 3);
    let second = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(second.work.cache.hits, 3);
    assert_eq!(second.work.cache.routing_hits, 3);
    assert_eq!(second.work.cache.routing_misses, 0);
    assert_eq!(second.work.evaluations.values().sum::<usize>(), 0);
    assert_eq!(first.assembly, second.assembly);
    assert_eq!(first.routes, second.routes);
    assert_eq!(first.metrics, second.metrics);
    validate_composed(&p, &second, &CanonicalPins::new(), &AtomicBool::new(false)).unwrap();
}
#[test]
fn changed_relationships_reroute_accepted_positions_and_revision_rebases_without_claim_loss() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    p.revision += 1;
    p.key.push_str("-revision-2");
    p.edges.push(Edge {
        bundle: None,
        id: "reverse".into(),
        source: "row".into(),
        target: "auth".into(),
        semantic_ids: vec!["reverse-claim".into()],
    });
    let next = run(&p, &ctx, Some((&ctx, &first)), &[], &mut cache).unwrap();
    assert_eq!(next.work.cache.previous_hits, 3);
    assert_eq!(next.work.cache.routing_hits, 0);
    assert_eq!(next.work.cache.routing_misses, 3);
    assert_eq!(next.routes.len(), 2);
    assert_eq!(next.routes["reverse"].edge, p.edges[1]);
    for id in first.assembly.frame_ids() {
        assert_eq!(
            first.assembly.frame(id).unwrap().solution().boxes,
            next.assembly.frame(id).unwrap().solution().boxes
        );
        assert_eq!(
            first.assembly.instance(id).unwrap(),
            next.assembly.instance(id).unwrap()
        );
        assert_eq!(next.assembly.frame(id).unwrap().solution().revision, 2);
    }
    p.revision = 1;
    assert!(
        run(&p, &ctx, None, &[], &mut cache)
            .unwrap_err()
            .to_string()
            .contains("predates")
    );
}

#[test]
fn route_membership_changes_recompute_complete_batches_with_unchanged_boxes() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    p.revision += 1;
    p.edges[0]
        .semantic_ids
        .push("another-original-coverage".into());
    let next = run(&p, &ctx, Some((&ctx, &first)), &[], &mut cache).unwrap();
    for id in first.assembly.frame_ids() {
        assert_eq!(
            first.assembly.instance(id).unwrap(),
            next.assembly.instance(id).unwrap()
        );
        assert_eq!(
            first.assembly.frame(id).unwrap().solution().boxes,
            next.assembly.frame(id).unwrap().solution().boxes
        );
    }
    assert_eq!(next.work.cache.previous_hits, 3);
    assert_eq!(next.work.cache.routing_hits, 0);
    assert_eq!(next.work.cache.routing_misses, 3);
    assert!(
        next.routes["auth-row"]
            .fragments
            .iter()
            .all(|fragment| fragment.route.semantic_ids == p.edges[0].semantic_ids)
    );
    validate_composed(&p, &next, &CanonicalPins::new(), &AtomicBool::new(false)).unwrap();
}

#[test]
fn changed_clearance_and_model_identity_never_reuse_stale_route_batches() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    let changed = solve_composed_cached(
        ComposedRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile {
                clearance: 1,
                ..QualityProfile::default()
            },
            pins: &CanonicalPins::new(),
            work_budget: 192,
            cancelled: &AtomicBool::new(false),
        },
        ComposedReuse {
            context: &ctx,
            previous: Some((&ctx, &first)),
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    assert_eq!(changed.work.cache.previous_hits, 3);
    assert_eq!(changed.work.cache.routing_hits, 0);
    assert_eq!(changed.work.cache.routing_misses, 3);
    validate_composed(&p, &changed, &CanonicalPins::new(), &AtomicBool::new(false)).unwrap();
    p.model_id = "another-model-with-the-same-ids".into();
    let next = run(&p, &context(&p), None, &[], &mut cache).unwrap();
    assert_eq!(next.work.cache.routing_hits, 0);
    assert_eq!(next.work.cache.routing_misses, 3);
}

#[test]
fn changed_parent_lanes_recompute_boundary_routes_and_match_uncached_routing() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    let old_auth = first.assembly.frame("root").unwrap().solution().boxes["auth"]
        .bounds
        .center();
    p.edges.push(Edge {
        bundle: None,
        id: "second-coverage".into(),
        source: "auth".into(),
        target: "row".into(),
        semantic_ids: vec!["second-coverage-claim".into()],
    });
    let pins = BTreeMap::from([(
        "root".into(),
        BTreeMap::from([("auth".into(), old_auth + Point::new(64, 0, 0))]),
    )]);
    let cancel = AtomicBool::new(false);
    let request = ComposedRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &pins,
        work_budget: 192,
        cancelled: &cancel,
    };
    let reuse = ComposedReuse {
        context: &ctx,
        previous: Some((&ctx, &first)),
        anchor_pins: &[],
    };
    let changed = solve_composed_cached(request, reuse, &mut cache).unwrap();
    let mut disabled = CanonicalPlacementCache::new(CanonicalCacheLimits {
        max_entries: 0,
        max_bytes: 0,
    })
    .unwrap();
    let reference = solve_composed_cached(request, reuse, &mut disabled).unwrap();
    assert_eq!(changed.assembly, reference.assembly);
    assert_eq!(changed.routes, reference.routes);
    assert_eq!(changed.metrics, reference.metrics);
    assert!(
        first.routes["auth-row"]
            .fragments
            .iter()
            .zip(&changed.routes["auth-row"].fragments)
            .any(|(old, new)| old.frame_id != "root" && old.request != new.request),
        "fixture must change an interior boundary terminal"
    );
    assert!(changed.work.cache.routing_misses >= 2);
    validate_composed(&p, &changed, &pins, &cancel).unwrap();
}

#[test]
fn cache_work_from_older_saved_states_defaults_new_routing_counters() {
    let old = serde_json::json!({"hits":1,"previous_hits":2,"misses":3,
        "evictions":0,"entries":6,"estimated_bytes":4096});
    let stats: visualyze_layout::composed::CanonicalCacheStats =
        serde_json::from_value(old).unwrap();
    assert_eq!(stats.routing_hits, 0);
    assert_eq!(stats.routing_misses, 0);
}
#[test]
fn changing_one_region_preserves_other_frames_and_enter_return_reuses_canonical_interiors() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    p.nodes
        .extend((0..8).map(|i| node(&format!("column-{i}"), Some("table"), true, 0)));
    let grown = run(&p, &ctx, Some((&ctx, &first)), &[], &mut cache).unwrap();
    assert_eq!(grown.work.cache.previous_hits, 2);
    for id in ["root", "db"] {
        assert_eq!(
            first.assembly.frame(id).unwrap().solution().boxes,
            grown.assembly.frame(id).unwrap().solution().boxes
        );
        assert_eq!(
            first.assembly.frame(id).unwrap().bounds(),
            grown.assembly.frame(id).unwrap().bounds()
        );
    }
    let mut focus = p.clone();
    focus.root = "table".into();
    focus.key = "entered-table".into();
    focus
        .nodes
        .retain(|node| node.id == "table" || node.parent.as_deref() == Some("table"));
    focus
        .nodes
        .iter_mut()
        .find(|node| node.id == "table")
        .unwrap()
        .parent = None;
    focus.edges.clear();
    let focus_ctx = context(&focus);
    let entered = run(&focus, &focus_ctx, Some((&ctx, &grown)), &[], &mut cache).unwrap();
    assert_eq!(entered.work.cache.hits, 1);
    assert_eq!(entered.assembly.root(), "table");
    assert_eq!(
        entered.assembly.frame("table").unwrap().solution().boxes,
        grown.assembly.frame("table").unwrap().solution().boxes
    );
    let returned = run(&p, &ctx, Some((&focus_ctx, &entered)), &[], &mut cache).unwrap();
    assert_eq!(returned.work.cache.hits, 3);
    assert_eq!(
        returned.present(&AtomicBool::new(false)).unwrap(),
        grown.present(&AtomicBool::new(false)).unwrap()
    );
}
#[test]
fn global_anchor_pin_keeps_chain_and_refuses_growth_that_would_rescale_it_transactionally() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    let pin = AnchorPin::capture(&first.assembly, "table", "root", "row").unwrap();
    let unchanged = run(
        &p,
        &ctx,
        Some((&ctx, &first)),
        std::slice::from_ref(&pin),
        &mut cache,
    )
    .unwrap();
    pin.validate(&unchanged.assembly).unwrap();
    let len = cache.len();
    let bytes = cache.estimated_bytes();
    // Symmetric neighboring bands let the existing repair initializer keep
    // the pinned middle layer legal, isolating the presentation-anchor rule.
    p.nodes.extend((0..24).map(|i| {
        node(
            &format!("column-{i}"),
            Some("table"),
            true,
            if i < 12 { -1 } else { 1 },
        )
    }));
    let error = run(&p, &ctx, Some((&ctx, &unchanged)), &[pin], &mut cache).unwrap_err();
    assert!(
        error.to_string().contains("Anchor-frame pin conflicts"),
        "{error}"
    );
    assert_eq!(cache.len(), len);
    assert_eq!(cache.estimated_bytes(), bytes);
    assert_eq!(first.assembly, unchanged.assembly);
}
#[test]
fn local_anchor_allows_ancestor_rescaling_while_keeping_its_canonical_object_fixed() {
    let mut p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    let pin = AnchorPin::capture(&first.assembly, "table", "table", "row").unwrap();
    // Symmetric neighboring bands let the existing repair initializer keep
    // the pinned middle layer legal, isolating the presentation-anchor rule.
    p.nodes.extend((0..24).map(|i| {
        node(
            &format!("column-{i}"),
            Some("table"),
            true,
            if i < 12 { -1 } else { 1 },
        )
    }));
    let grown = run(
        &p,
        &ctx,
        Some((&ctx, &first)),
        std::slice::from_ref(&pin),
        &mut cache,
    )
    .unwrap();
    pin.validate(&grown.assembly).unwrap();
    assert!(
        grown
            .assembly
            .instance("table")
            .unwrap()
            .transform
            .scale_shift()
            > first
                .assembly
                .instance("table")
                .unwrap()
                .transform
                .scale_shift()
    );
}

#[test]
fn cache_limits_evict_deterministically_and_disabled_cache_stays_bounded() {
    let p = fixture();
    let ctx = context(&p);
    let limits = CanonicalCacheLimits {
        max_entries: 1,
        max_bytes: 1024 * 1024,
    };
    let mut cache = CanonicalPlacementCache::new(limits).unwrap();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(first.work.cache.evictions, 2);
    assert_eq!(cache.len(), 1);
    assert!(cache.estimated_bytes() <= limits.max_bytes);
    let second = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(second.work.cache.hits, 1);
    assert_eq!(second.work.cache.misses, 2);
    assert_eq!(second.work.cache.routing_hits, 1);
    assert_eq!(second.work.cache.routing_misses, 2);
    let mut disabled = CanonicalPlacementCache::new(CanonicalCacheLimits {
        max_entries: 8,
        max_bytes: 1,
    })
    .unwrap();
    run(&p, &ctx, None, &[], &mut disabled).unwrap();
    assert!(disabled.is_empty());
    assert_eq!(disabled.estimated_bytes(), 0);
}
#[test]
fn allowed_domains_and_typed_lenses_invalidate_hits_without_freezing_winning_dimensions() {
    let mut p = fixture();
    let mut ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    run(&p, &ctx, None, &[], &mut cache).unwrap();
    p.nodes
        .iter_mut()
        .find(|node| node.id == "row")
        .unwrap()
        .max_size
        .x += 1;
    let changed = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(changed.work.cache.hits, 2);
    assert_eq!(changed.work.cache.misses, 1);
    ctx.lens = SemanticLens::Code;
    let other = run(&p, &ctx, None, &[], &mut cache).unwrap();
    assert_eq!(other.work.cache.hits, 0);
    assert_eq!(other.work.cache.misses, 3);
}
#[test]
fn cancelled_or_inconsistent_context_never_publishes_cache_mutations_and_zero_budget_can_reuse_previous()
 {
    let p = fixture();
    let ctx = context(&p);
    let mut cache = CanonicalPlacementCache::default();
    let first = run(&p, &ctx, None, &[], &mut cache).unwrap();
    let before = (cache.len(), cache.estimated_bytes());
    let pins = CanonicalPins::new();
    let request = ComposedRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &pins,
        work_budget: 0,
        cancelled: &AtomicBool::new(false),
    };
    let reuse = ComposedReuse {
        context: &ctx,
        previous: Some((&ctx, &first)),
        anchor_pins: &[],
    };
    let cached = solve_composed_cached(request, reuse, &mut cache).unwrap();
    assert_eq!(cached.work.evaluations.values().sum::<usize>(), 0);
    let stable = (cache.len(), cache.estimated_bytes());
    assert_eq!(
        solve_composed_cached(
            ComposedRequest {
                cancelled: &AtomicBool::new(true),
                ..request
            },
            reuse,
            &mut cache
        ),
        Err(LayoutError::Cancelled)
    );
    assert_eq!((cache.len(), cache.estimated_bytes()), stable);
    let mut bad = ctx.clone();
    bad.root = "wrong-root".into();
    assert!(run(&p, &bad, None, &[], &mut cache).is_err());
    assert_eq!((cache.len(), cache.estimated_bytes()), stable);
    assert!(stable.0 >= before.0);
}
