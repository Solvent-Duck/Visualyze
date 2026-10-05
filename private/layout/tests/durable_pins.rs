use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::{AnchorPin, DyadicTransform, ExactPoint},
    composed::{
        CanonicalCacheLimits, CanonicalPins, CanonicalPlacementCache, ComposedRequest,
        ComposedReuse, CompositionContext, solve_composed_cached, solve_composed_reorganized,
        validate_composed,
    },
    projection::LayoutPolicy,
};
use visualyze_model::provider::SemanticLens;

fn fixture() -> (Projection, CompositionContext) {
    let node = |id: &str, parent: Option<&str>, solid: bool, band| Node {
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
    };
    let p = Projection {
        key: "durable-pin-view".into(),
        model_id: "durable-model".into(),
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
    cancelled: &'a AtomicBool,
) -> ComposedRequest<'a> {
    ComposedRequest {
        projection: p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins,
        work_budget: 192,
        cancelled,
    }
}
#[test]
fn durable_pin_records_roundtrip_and_reject_corrupt_exact_chains() {
    let (p, context) = fixture();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let mut cache = CanonicalPlacementCache::default();
    let old = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    for anchor in ["root", "table"] {
        let pin = AnchorPin::capture(&old.assembly, "table", anchor, "row").unwrap();
        let restored = AnchorPin::from_record(pin.to_record()).unwrap();
        assert_eq!(pin, restored);
        restored.validate(&old.assembly).unwrap();
    }
    let pin = AnchorPin::capture(&old.assembly, "table", "root", "row").unwrap();
    for mutation in 0..9 {
        let mut record = pin.to_record();
        match mutation {
            0 => record.links[0].parent = "table".into(),
            1 => record.links.reverse(),
            2 => record.anchored_point = ExactPoint::integer(Point::new(999, 999, 999)).unwrap(),
            3 => record.links[0].envelope.max.x += 1,
            4 => record.members.push(record.members[0].clone()),
            5 => record.owner = "root".into(),
            6 => record.links[0].canonical_bounds.min.x = i64::MIN,
            7 => record.links[0].transform = DyadicTransform::identity(),
            _ => record.object_id = "x".repeat(4097),
        }
        assert!(
            AnchorPin::from_record(record).is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn evicted_previous_geometry_is_not_required_to_recover_exact_anchor_locks() {
    let (p, context) = fixture();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let mut cache = CanonicalPlacementCache::default();
    let old = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    let pin = AnchorPin::from_record(
        AnchorPin::capture(&old.assembly, "table", "root", "row")
            .unwrap()
            .to_record(),
    )
    .unwrap();
    let mut empty = CanonicalPlacementCache::new(CanonicalCacheLimits {
        max_entries: 0,
        max_bytes: 0,
    })
    .unwrap();
    let reuse = ComposedReuse {
        context: &context,
        previous: None,
        anchor_pins: std::slice::from_ref(&pin),
    };
    let recovered = solve_composed_cached(request(&p, &pins, &cancel), reuse, &mut empty).unwrap();
    pin.validate(&recovered.assembly).unwrap();
    validate_composed(&p, &recovered, &pins, &cancel).unwrap();
    assert!(recovered.work.evaluations.values().all(|n| *n > 0));
    assert!(
        recovered.work.evaluations.values().sum::<usize>() + recovered.work.feedback_evaluations
            <= 192
    );
    assert_eq!(recovered.work.cache.hits, 0);
    assert!(empty.is_empty());
    let repeated = solve_composed_cached(request(&p, &pins, &cancel), reuse, &mut empty).unwrap();
    assert_eq!(recovered, repeated);
    let searched = solve_composed_reorganized(request(&p, &pins, &cancel), reuse, &empty).unwrap();
    pin.validate(&searched.assembly).unwrap();
    assert!(searched.work.evaluations.values().all(|n| *n > 0));
    let domain = old.assembly.frame("table").unwrap().bounds();
    assert_eq!(recovered.assembly.frame("table").unwrap().bounds(), domain);
    assert_eq!(
        recovered.assembly.instance("table").unwrap(),
        old.assembly.instance("table").unwrap()
    );
    assert!(
        solve_composed_cached(
            ComposedRequest {
                work_budget: 0,
                ..request(&p, &pins, &cancel)
            },
            reuse,
            &mut empty
        )
        .is_err()
    );
}
#[test]
fn recovery_rejects_changed_membership_ancestry_dimensions_and_conflicting_pins() {
    let (p, context) = fixture();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let mut cache = CanonicalPlacementCache::default();
    let old = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    let pin = AnchorPin::capture(&old.assembly, "table", "root", "row").unwrap();
    let reuse = ComposedReuse {
        context: &context,
        previous: None,
        anchor_pins: std::slice::from_ref(&pin),
    };
    let before = (cache.len(), cache.estimated_bytes());
    for mutation in 0..4 {
        let mut changed = p.clone();
        match mutation {
            0 => {
                changed
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == "row")
                    .unwrap()
                    .semantic_ids = vec!["different-member".into()]
            }
            1 => {
                changed
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == "table")
                    .unwrap()
                    .parent = Some("root".into())
            }
            2 => {
                let node = changed.nodes.iter_mut().find(|n| n.id == "table").unwrap();
                node.preferred_size = node.min_size;
                node.max_size = node.min_size;
            }
            _ => {
                changed.nodes.retain(|n| n.id != "row");
                changed.edges.clear();
            }
        }
        assert!(
            solve_composed_cached(request(&changed, &pins, &cancel), reuse, &mut cache).is_err(),
            "mutation {mutation}"
        );
        assert_eq!((cache.len(), cache.estimated_bytes()), before);
    }
    let conflict = BTreeMap::from([(
        "table".into(),
        BTreeMap::from([("row".into(), Point::new(999, 0, 0))]),
    )]);
    assert!(solve_composed_cached(request(&p, &conflict, &cancel), reuse, &mut cache).is_err());
    assert_eq!(
        solve_composed_cached(
            ComposedRequest {
                cancelled: &AtomicBool::new(true),
                ..request(&p, &pins, &cancel)
            },
            reuse,
            &mut cache
        ),
        Err(LayoutError::Cancelled)
    );
    assert_eq!((cache.len(), cache.estimated_bytes()), before);
}

#[test]
fn individually_valid_records_cannot_disagree_about_a_shared_transform() {
    let (p, context) = fixture();
    let pins = CanonicalPins::new();
    let cancel = AtomicBool::new(false);
    let mut cache = CanonicalPlacementCache::default();
    let old = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[],
        },
        &mut cache,
    )
    .unwrap();
    let pin = AnchorPin::capture(&old.assembly, "table", "root", "row").unwrap();
    let mut record = pin.to_record();
    let shifted = |point: ExactPoint| {
        let mut values = point.numerators();
        values[0] += 20i128 << point.denominator_shift();
        ExactPoint::new(values, point.denominator_shift()).unwrap()
    };
    let link = record.links.last_mut().unwrap();
    link.envelope.min.x += 20;
    link.envelope.max.x += 20;
    link.transform = DyadicTransform::from_exact(
        link.transform.origin(),
        shifted(link.transform.translation()),
        link.transform.scale_shift(),
    )
    .unwrap();
    record.anchored_point = shifted(record.anchored_point);
    let contradictory = AnchorPin::from_record(record).unwrap();
    let before = (cache.len(), cache.estimated_bytes());
    let error = solve_composed_cached(
        request(&p, &pins, &cancel),
        ComposedReuse {
            context: &context,
            previous: None,
            anchor_pins: &[pin, contradictory],
        },
        &mut cache,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("conflicting frame locks"), "{error}");
    assert_eq!((cache.len(), cache.estimated_bytes()), before);
}
