use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    Algorithm, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::{AnchorPin, Assembly, DyadicTransform, ExactPoint},
    composed::{
        CanonicalPins, CanonicalPlacementCache, ComposedRequest, ComposedReuse, ComposedSolution,
        CompositionContext,
        persistence::{
            PersistedConstraintSet, PinPresentation, PinReference, validate_constraint_registry,
        },
        resolve_constraints, solve_composed, solve_composed_cached,
    },
    projection::{DisplayGroup, LayoutPolicy},
};
use visualyze_model::provider::SemanticLens;

fn context(root: &str) -> CompositionContext {
    CompositionContext {
        model_id: "inherited-fixture".into(),
        root: root.into(),
        lens: SemanticLens::Logical,
        policy: LayoutPolicy::Crud,
    }
}
fn node(id: &str, parent: Option<&str>, solid: bool) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        kind: if solid {
            NodeKind::Service
        } else {
            NodeKind::Scope
        },
        band: 0,
        solid,
        min_size: Point::new(12, 8, 12),
        preferred_size: Point::new(24, 16, 24),
        max_size: Point::new(48, 32, 48),
    }
}
fn fixture() -> (Projection, ComposedSolution, PersistedConstraintSet) {
    let p = Projection {
        key: "inherited-fixture".into(),
        model_id: context("root").model_id,
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false),
            node("auth", Some("root"), true),
            node("app", Some("root"), false),
            node("inner", Some("app"), false),
            node("leaf", Some("inner"), true),
        ],
        edges: vec![],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let mut s = solve_composed(ComposedRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &CanonicalPins::new(),
        work_budget: 96,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    // Preserve the affine map but require an exact fractional translation and
    // a nonzero transform origin. The resolver must not round either away.
    let frames = s
        .assembly
        .frame_ids()
        .map(|id| s.assembly.frame(id).unwrap().clone())
        .collect();
    let mut instances: Vec<_> = s
        .assembly
        .frame_ids()
        .map(|id| s.assembly.instance(id).unwrap().clone())
        .collect();
    let child = instances
        .iter_mut()
        .find(|instance| instance.frame_id == "inner")
        .unwrap();
    let old = child.transform;
    let shift = old.scale_shift();
    assert!(shift > 0);
    let translation = old.translation().to_integer().unwrap();
    let mut numerators =
        [translation.x, translation.y, translation.z].map(|n| i128::from(n) << shift);
    numerators[0] += 1;
    child.transform = DyadicTransform::from_exact(
        old.origin() + Point::new(1, 0, 0),
        ExactPoint::new(numerators, shift).unwrap(),
        shift,
    )
    .unwrap();
    s.assembly = Assembly::new(frames, instances).unwrap();
    let pin = AnchorPin::capture(&s.assembly, "inner", "root", "leaf").unwrap();
    let set = PersistedConstraintSet {
        presentation: BTreeMap::new(),
        context: context("root"),
        pins: BTreeMap::from([(
            "inner".into(),
            BTreeMap::from([("leaf".into(), pin.local_point())]),
        )]),
        anchors: vec![pin],
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
        )]),
    };
    (p, s, set)
}
fn resolve(root: &str, registry: &[PersistedConstraintSet]) -> PersistedConstraintSet {
    resolve_constraints(&context(root), registry, &AtomicBool::new(false)).unwrap()
}
fn entered(p: &Projection, root: &str) -> Projection {
    let mut p = p.clone();
    p.key = format!("entered:{root}");
    p.root = root.into();
    p.nodes
        .retain(|node| [root, "inner", "leaf"].contains(&node.id.as_str()));
    p.nodes
        .iter_mut()
        .find(|node| node.id == root)
        .unwrap()
        .parent = None;
    p
}

#[test]
fn descent_truncates_exact_dyadics_and_preserves_cold_solve_geometry() {
    let (p, accepted, source) = fixture();
    let original = source.clone();
    for root in ["app", "inner"] {
        let effective = resolve(root, std::slice::from_ref(&source));
        assert_eq!(effective.pins, source.pins);
        assert_eq!(effective.ownership, source.ownership);
        assert_eq!(effective.anchors.len(), 1);
        let pin = &effective.anchors[0];
        let expected = AnchorPin::capture(&accepted.assembly, "inner", root, "leaf").unwrap();
        assert_eq!(pin, &expected);
        if root == "app" {
            assert_ne!(
                pin.links()[0].transform.translation().denominator_shift(),
                0
            );
        }
        let p = entered(&p, root);
        let solved = solve_composed_cached(
            ComposedRequest {
                projection: &p,
                algorithm: Algorithm::Floorplan,
                quality: QualityProfile::default(),
                pins: &effective.pins,
                work_budget: 96,
                cancelled: &AtomicBool::new(false),
            },
            ComposedReuse {
                context: &effective.context,
                previous: None,
                anchor_pins: &effective.anchors,
            },
            &mut CanonicalPlacementCache::default(),
        )
        .unwrap();
        pin.validate(&solved.assembly).unwrap();
        assert_eq!(
            solved.assembly.frame("inner").unwrap().solution().boxes["leaf"]
                .bounds
                .center(),
            pin.local_point()
        );
    }
    assert_eq!(
        source, original,
        "resolution must not mutate source-owned records"
    );
}

#[test]
fn shorter_region_anchors_and_local_order_are_preserved() {
    let (_, s, mut source) = fixture();
    source.anchors = vec![AnchorPin::capture(&s.assembly, "inner", "inner", "leaf").unwrap()];
    let effective = resolve("app", &[source.clone()]);
    assert_eq!(effective.anchors, source.anchors);
    let mut local = effective.clone();
    local.anchors.insert(
        0,
        AnchorPin::capture(&s.assembly, "inner", "app", "leaf").unwrap(),
    );
    assert_eq!(resolve("app", &[source.clone(), local.clone()]), local);
    assert_eq!(resolve("app", &[local.clone(), source]), local);
}

#[test]
fn child_origin_pins_keep_exact_short_anchors_in_a_cold_ancestor_solve() {
    let (p, accepted, source) = fixture();
    let child = resolve("app", std::slice::from_ref(&source));
    assert_ne!(
        child.anchors[0].links()[0]
            .transform
            .translation()
            .denominator_shift(),
        0
    );
    let original = child.clone();
    let effective = resolve("root", std::slice::from_ref(&child));
    assert_eq!(effective.pins, child.pins);
    assert_eq!(effective.anchors, child.anchors);
    assert_eq!(effective.ownership, child.ownership);
    assert_eq!(effective.anchors[0].anchor(), "app");
    assert_eq!(
        effective.anchors[0].links().len(),
        1,
        "no source-to-parent lock was invented"
    );
    let solved = solve_composed_cached(
        ComposedRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins: &effective.pins,
            work_budget: 96,
            cancelled: &AtomicBool::new(false),
        },
        ComposedReuse {
            context: &effective.context,
            previous: None,
            anchor_pins: &effective.anchors,
        },
        &mut CanonicalPlacementCache::default(),
    )
    .unwrap();
    effective.anchors[0].validate(&solved.assembly).unwrap();
    assert_eq!(
        solved.assembly.instance("inner").unwrap(),
        accepted.assembly.instance("inner").unwrap()
    );
    assert_eq!(
        solved.assembly.frame("inner").unwrap().bounds(),
        accepted.assembly.frame("inner").unwrap().bounds()
    );
    assert_eq!(child, original);

    // A pin created in its owning frame has no presentation links to extend.
    let inner = resolve("inner", std::slice::from_ref(&source));
    let parent = resolve("root", std::slice::from_ref(&inner));
    assert_eq!(parent.anchors, inner.anchors);
    assert!(parent.anchors[0].links().is_empty());
}

#[test]
fn upward_and_downward_constraints_merge_deterministically_without_reowning() {
    let (_, _, root) = fixture();
    let child = resolve("app", std::slice::from_ref(&root));
    let leaf = resolve("inner", std::slice::from_ref(&root));
    for target in ["root", "app", "inner"] {
        let expected = resolve(target, &[root.clone(), child.clone(), leaf.clone()]);
        for registry in [
            vec![leaf.clone(), child.clone(), root.clone()],
            vec![child.clone(), root.clone(), leaf.clone()],
        ] {
            assert_eq!(resolve(target, &registry), expected);
        }
        assert_eq!(expected.pins, root.pins);
        assert_eq!(expected.ownership, root.ownership);
    }
    let effective = resolve("root", &[child.clone(), leaf.clone()]);
    assert_eq!(effective.anchors.len(), 2);
    assert_eq!(child.context.root, "app");
    assert_eq!(leaf.context.root, "inner");
}

#[test]
fn upward_constraints_reject_conflicts_and_do_not_cross_unknown_paths_or_lenses() {
    let (_, _, root) = fixture();
    let child = resolve("app", std::slice::from_ref(&root));
    for mutation in 0..4 {
        let mut changed = child.clone();
        match mutation {
            0 => {
                changed.anchors.clear();
                changed
                    .pins
                    .get_mut("inner")
                    .unwrap()
                    .get_mut("leaf")
                    .unwrap()
                    .x += 1;
            }
            1 => {
                let mut record = changed.anchors[0].to_record();
                record.members = vec!["other-semantic-object".into()];
                changed.anchors[0] = AnchorPin::from_record(record).unwrap();
            }
            2 => changed
                .ownership
                .get_mut("leaf")
                .unwrap()
                .push("changed-top".into()),
            _ => changed.context.policy = LayoutPolicy::Peers,
        }
        assert!(
            resolve_constraints(
                &context("root"),
                &[root.clone(), changed],
                &AtomicBool::new(false)
            )
            .is_err(),
            "mutation {mutation}"
        );
    }
    for mutation in 0..4 {
        let mut excluded = child.clone();
        match mutation {
            0 => excluded.ownership.clear(),
            1 => excluded.context.lens = SemanticLens::Code,
            2 => excluded.context.root = "unrelated".into(),
            _ => {
                // A source cannot export a coordinate frame above itself.
                excluded.anchors.clear();
                excluded.pins = BTreeMap::from([(
                    "root".into(),
                    BTreeMap::from([("leaf".into(), Point::ZERO)]),
                )]);
            }
        }
        assert!(
            resolve("root", &[excluded]).pins.is_empty(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn unrelated_models_lenses_paths_and_owners_do_not_inherit() {
    let (_, _, source) = fixture();
    for mutation in 0..5 {
        let mut source = source.clone();
        match mutation {
            0 => source.context.model_id = "other-model".into(),
            1 => source.context.lens = SemanticLens::Code,
            2 => source.ownership.clear(),
            3 => source.context.root = "not-an-ancestor".into(),
            _ => {
                source.ownership.get_mut("leaf").unwrap().remove(2);
            }
        };
        assert!(resolve("app", &[source]).pins.is_empty());
    }
    // The leaf is under the target but its canonical owner need not be. Do
    // not transplant a parent's coordinate system into the entered frame.
    let mut outside_owner = source.clone();
    outside_owner.anchors.clear();
    outside_owner.pins = BTreeMap::from([(
        "root".into(),
        BTreeMap::from([("leaf".into(), Point::ZERO)]),
    )]);
    assert!(resolve("app", &[outside_owner]).pins.is_empty());
    let mut group = source;
    group.ownership.clear();
    group.pins = BTreeMap::from([(
        "inner".into(),
        BTreeMap::from([("group:looks-supplied".into(), Point::ZERO)]),
    )]);
    group.anchors.clear();
    assert!(resolve("app", &[group.clone()]).pins.is_empty());
    assert_eq!(resolve("root", &[group.clone()]), group);
}

#[test]
fn policy_mismatch_is_an_explicit_conflict_only_when_applicable() {
    let (_, _, mut source) = fixture();
    source.context.policy = LayoutPolicy::Peers;
    let error = resolve_constraints(&context("app"), &[source.clone()], &AtomicBool::new(false))
        .unwrap_err()
        .to_string();
    assert!(error.contains("policy"));
    assert!(resolve("unrelated", &[source.clone()]).pins.is_empty());
    source.context.lens = SemanticLens::Code;
    assert!(resolve("app", &[source]).pins.is_empty());
}

#[test]
fn merging_is_order_independent_and_deduplicates_identical_constraints() {
    let (_, _, source) = fixture();
    let local = resolve("app", std::slice::from_ref(&source));
    let mut ancestor = source.clone();
    ancestor.context.root = "outer".into();
    ancestor
        .ownership
        .get_mut("leaf")
        .unwrap()
        .push("outer".into());
    // Different supplied paths are intentionally conflicts, even if their
    // overlapping suffix appears compatible. Real records retain full paths.
    let mut source = source;
    source.ownership = ancestor.ownership.clone();
    let mut local = local;
    local.ownership = source.ownership.clone();
    let expected = resolve("inner", &[source.clone(), local.clone(), ancestor.clone()]);
    for sets in [
        vec![ancestor.clone(), source.clone(), local.clone()],
        vec![local, ancestor, source],
    ] {
        assert_eq!(resolve("inner", &sets), expected);
    }
    assert_eq!(expected.anchors.len(), 1);
    assert_eq!(expected.pins.len(), 1);
}

#[test]
fn incompatible_coordinates_membership_ownership_and_frame_signatures_fail() {
    let (_, _, source) = fixture();
    for mutation in 0..4 {
        let mut local = resolve("app", std::slice::from_ref(&source));
        match mutation {
            0 => {
                local.anchors.clear();
                local
                    .pins
                    .get_mut("inner")
                    .unwrap()
                    .get_mut("leaf")
                    .unwrap()
                    .x += 1;
            }
            1 => {
                let mut record = local.anchors[0].to_record();
                record.members = vec!["changed-member".into()];
                local.anchors[0] = AnchorPin::from_record(record).unwrap();
            }
            2 => local
                .ownership
                .get_mut("leaf")
                .unwrap()
                .insert(3, "changed-owner".into()),
            _ => {
                let mut record = local.anchors[0].to_record();
                let transform = record.links[0].transform;
                let shift = transform.scale_shift();
                let old = transform.translation();
                let mut numerators = old
                    .numerators()
                    .map(|n| n << (shift - old.denominator_shift()));
                numerators[0] += 1;
                record.links[0].transform = DyadicTransform::from_exact(
                    transform.origin() + Point::new(1, 0, 0),
                    ExactPoint::new(numerators, shift).unwrap(),
                    shift,
                )
                .unwrap();
                local.anchors[0] = AnchorPin::from_record(record).unwrap();
            }
        }
        for registry in [
            vec![source.clone(), local.clone()],
            vec![local, source.clone()],
        ] {
            assert!(
                resolve_constraints(&context("app"), &registry, &AtomicBool::new(false)).is_err(),
                "mutation {mutation}"
            );
        }
    }
}

#[test]
fn contradictory_anchor_chain_cannot_skip_the_target() {
    let (_, _, mut source) = fixture();
    let mut record = source.anchors[0].to_record();
    record.links.truncate(1);
    record.links[0].parent = "root".into();
    record.anchored_point = record.links[0]
        .transform
        .map_point(record.local_point)
        .unwrap();
    source.anchors[0] = AnchorPin::from_record(record).unwrap();
    let error = resolve_constraints(&context("app"), &[source], &AtomicBool::new(false))
        .unwrap_err()
        .to_string();
    assert!(error.contains("anchor chain"));
}

#[test]
fn cancellation_context_identity_and_registry_limits_are_checked() {
    let (_, _, source) = fixture();
    assert!(matches!(
        resolve_constraints(&context("app"), &[], &AtomicBool::new(true)),
        Err(LayoutError::Cancelled)
    ));
    assert!(
        resolve_constraints(
            &context("app"),
            &vec![source.clone(); 65],
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(
        resolve_constraints(
            &context("app"),
            &[source.clone(), source],
            &AtomicBool::new(false)
        )
        .is_err()
    );
    let mut invalid = context("app");
    invalid.root.clear();
    assert!(resolve_constraints(&invalid, &[], &AtomicBool::new(false)).is_err());
}

fn group_fixture() -> (Projection, ComposedSolution, PersistedConstraintSet) {
    let (mut projection, accepted, mut source) = fixture();
    // `inner` is explicitly declared as a visual frame. Its spelling carries
    // no type information, and supplied ownership still goes straight to app.
    let group = DisplayGroup {
        id: "inner".into(),
        parent: "app".into(),
        members: vec!["leaf".into(), "other".into()],
        suggested: true,
    };
    let frame = projection
        .nodes
        .iter_mut()
        .find(|n| n.id == "inner")
        .unwrap();
    frame.kind = NodeKind::Group;
    frame.semantic_ids = group.members.clone();
    projection.suggested_groups = vec![group.clone()];
    source.ownership.insert(
        "leaf".into(),
        vec!["leaf".into(), "app".into(), "root".into()],
    );
    source.presentation.insert(
        "leaf".into(),
        PinPresentation {
            path: ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
            groups: vec![group],
            reference: None,
            supplied: BTreeMap::from([
                (
                    "leaf".into(),
                    ["leaf", "app", "root"].map(String::from).to_vec(),
                ),
                (
                    "other".into(),
                    ["other", "app", "root"].map(String::from).to_vec(),
                ),
                ("app".into(), ["app", "root"].map(String::from).to_vec()),
            ]),
        },
    );
    (projection, accepted, source)
}

#[test]
fn protected_group_frames_inherit_exactly_in_both_directions_and_cold_solve() {
    let (p, accepted, source) = group_fixture();
    let original = source.clone();
    for root in ["app", "inner"] {
        let effective = resolve(root, std::slice::from_ref(&source));
        assert_eq!(effective.ownership, source.ownership);
        assert_eq!(effective.presentation, source.presentation);
        assert_eq!(
            effective.anchors[0],
            AnchorPin::capture(&accepted.assembly, "inner", root, "leaf").unwrap()
        );
        let projection = entered(&p, root);
        let solved = solve_composed_cached(
            ComposedRequest {
                projection: &projection,
                algorithm: Algorithm::Floorplan,
                quality: QualityProfile::default(),
                pins: &effective.pins,
                work_budget: 96,
                cancelled: &AtomicBool::new(false),
            },
            ComposedReuse {
                context: &effective.context,
                previous: None,
                anchor_pins: &effective.anchors,
            },
            &mut CanonicalPlacementCache::default(),
        )
        .unwrap();
        effective.anchors[0].validate(&solved.assembly).unwrap();
        let upward = resolve("root", std::slice::from_ref(&effective));
        assert_eq!(upward.anchors, effective.anchors);
        assert_eq!(upward.presentation, effective.presentation);
        assert_eq!(upward.ownership, source.ownership);
    }
    assert_eq!(
        source, original,
        "effective constraints do not mutate their origin"
    );
}

#[test]
fn directly_pinned_group_preserves_aggregate_members_and_never_becomes_supplied() {
    let (_, accepted, mut source) = group_fixture();
    let mut record = AnchorPin::capture(&accepted.assembly, "app", "root", "inner")
        .unwrap()
        .to_record();
    record.members = vec!["leaf".into(), "other".into()];
    let pin = AnchorPin::from_record(record).unwrap();
    source.pins = BTreeMap::from([(
        "app".into(),
        BTreeMap::from([("inner".into(), pin.local_point())]),
    )]);
    source.anchors = vec![pin];
    source.ownership.clear();
    let mut metadata = source.presentation.remove("leaf").unwrap();
    metadata.path.remove(0);
    source.presentation.insert("inner".into(), metadata);
    let child = resolve("app", std::slice::from_ref(&source));
    assert_eq!(child.pins, source.pins);
    assert!(child.ownership.is_empty());
    assert_eq!(
        child.anchors[0].semantic_ids(),
        source.anchors[0].semantic_ids()
    );
    assert!(
        resolve("inner", std::slice::from_ref(&source))
            .pins
            .is_empty(),
        "the parent proxy does not become the canonical root"
    );
    let upward = resolve("root", std::slice::from_ref(&child));
    assert_eq!(upward.anchors, child.anchors);
    assert_eq!(upward.presentation, source.presentation);
}

#[test]
fn reference_pin_stays_with_its_origin_and_never_pins_the_real_target_frame() {
    let (_, _, source) = fixture();
    let mut reference = resolve("inner", &[source]);
    reference.ownership.clear();
    let mut record = reference.anchors[0].to_record();
    record.members = vec!["app".into()];
    reference.anchors[0] = AnchorPin::from_record(record).unwrap();
    reference.presentation.insert(
        "leaf".into(),
        PinPresentation {
            path: ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
            groups: vec![],
            reference: Some(PinReference {
                origin: "inner".into(),
                target: "app".into(),
            }),
            supplied: BTreeMap::from([
                (
                    "inner".into(),
                    ["inner", "app", "root"].map(String::from).to_vec(),
                ),
                ("app".into(), ["app", "root"].map(String::from).to_vec()),
            ]),
        },
    );
    let original = reference.clone();
    assert_eq!(resolve("inner", &[reference.clone()]), reference);
    for root in ["root", "app", "leaf", "other-origin"] {
        assert!(
            resolve(root, &[reference.clone()]).pins.is_empty(),
            "unexpected reference inheritance to {root}"
        );
    }
    assert_eq!(reference, original);
    let mut other_lens = reference;
    other_lens.context.lens = SemanticLens::Code;
    assert!(resolve("root", &[other_lens]).pins.is_empty());
}

#[test]
fn group_metadata_is_deterministic_and_conflicting_definitions_are_rejected() {
    let (_, _, source) = group_fixture();
    let mut child = resolve("app", std::slice::from_ref(&source));
    child.presentation.get_mut("leaf").unwrap().groups[0]
        .members
        .reverse();
    let expected = resolve("inner", &[source.clone(), child.clone()]);
    assert_eq!(resolve("inner", &[child.clone(), source.clone()]), expected);
    assert_eq!(expected.presentation, source.presentation);
    let metadata = child.presentation.get_mut("leaf").unwrap();
    metadata.groups[0].members = vec!["leaf".into(), "third".into()];
    metadata.supplied.remove("other");
    metadata.supplied.insert(
        "third".into(),
        ["third", "app", "root"].map(String::from).to_vec(),
    );
    validate_constraint_registry(
        &source.context.model_id,
        &[child.clone()],
        &AtomicBool::new(false),
    )
    .unwrap();
    for registry in [vec![source.clone(), child.clone()], vec![child, source]] {
        let error =
            resolve_constraints(&context("root"), &registry, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("conflicting protected group"));
    }
}

#[test]
fn malformed_or_orphan_presentation_evidence_is_rejected() {
    let (_, _, source) = group_fixture();
    for mutation in 0..9 {
        let mut invalid = source.clone();
        let metadata = invalid.presentation.get_mut("leaf").unwrap();
        let orphan = metadata.clone();
        match mutation {
            0 => metadata.path.swap(0, 1),
            1 => metadata.path.push("root".into()),
            2 => {
                metadata.supplied.remove("other");
            }
            3 => metadata.supplied.get_mut("other").unwrap()[1] = "wrong-parent".into(),
            4 => metadata.groups[0].members.push("leaf".into()),
            5 => metadata.groups[0].id = "not-on-path".into(),
            6 => {
                metadata.path.remove(1);
            }
            7 => {
                invalid.presentation.insert("orphan".into(), orphan);
            }
            _ => {
                metadata
                    .supplied
                    .insert("unrelated".into(), vec!["unrelated".into()]);
            }
        };
        assert!(
            validate_constraint_registry(
                &source.context.model_id,
                &[invalid],
                &AtomicBool::new(false)
            )
            .is_err(),
            "accepted mutation {mutation}"
        );
    }
    assert!(matches!(
        resolve_constraints(&context("inner"), &[source], &AtomicBool::new(true)),
        Err(LayoutError::Cancelled)
    ));
}

fn many_groups(count: usize, members: usize, padding: usize) -> PersistedConstraintSet {
    let mut set = PersistedConstraintSet {
        context: context("root"),
        pins: BTreeMap::new(),
        anchors: vec![],
        ownership: BTreeMap::new(),
        presentation: BTreeMap::new(),
    };
    for index in 0..count {
        let id = format!("group-{index}");
        let members: Vec<_> = (0..members)
            .map(|m| format!("member-{index}-{m}{}", "a".repeat(padding)))
            .collect();
        let mut supplied: BTreeMap<_, _> = members
            .iter()
            .map(|m| (m.clone(), vec![m.clone(), "root".into()]))
            .collect();
        supplied.insert("root".into(), vec!["root".into()]);
        set.pins
            .entry("root".into())
            .or_default()
            .insert(id.clone(), Point::ZERO);
        set.presentation.insert(
            id.clone(),
            PinPresentation {
                path: vec![id.clone(), "root".into()],
                groups: vec![DisplayGroup {
                    id,
                    parent: "root".into(),
                    members,
                    suggested: true,
                }],
                reference: None,
                supplied,
            },
        );
    }
    set
}

#[test]
fn protected_definition_member_and_shared_text_budgets_are_enforced_before_resolution() {
    let cancel = AtomicBool::new(false);
    let valid = many_groups(64, 2, 0);
    validate_constraint_registry(
        &valid.context.model_id,
        std::slice::from_ref(&valid),
        &cancel,
    )
    .unwrap();
    assert_eq!(resolve("root", &[valid]).presentation.len(), 64);
    for invalid in [
        many_groups(65, 2, 0),
        many_groups(1, 129, 0),
        many_groups(6, 128, 4000),
    ] {
        assert!(resolve_constraints(&context("root"), &[invalid], &cancel).is_err());
    }
}

#[test]
fn identical_reference_ids_in_different_origins_remain_independently_owned() {
    fn reference(origin: &str, ancestors: &[&str], x: i64) -> PersistedConstraintSet {
        let origin_path: Vec<_> = std::iter::once(origin)
            .chain(ancestors.iter().copied())
            .map(String::from)
            .collect();
        let path = std::iter::once("same-display-id".into())
            .chain(origin_path.clone())
            .collect();
        PersistedConstraintSet {
            context: context(origin),
            pins: BTreeMap::from([(
                origin.into(),
                BTreeMap::from([("same-display-id".into(), Point::new(x, 0, 0))]),
            )]),
            anchors: vec![],
            ownership: BTreeMap::new(),
            presentation: BTreeMap::from([(
                "same-display-id".into(),
                PinPresentation {
                    path,
                    groups: vec![],
                    reference: Some(PinReference {
                        origin: origin.into(),
                        target: "external".into(),
                    }),
                    supplied: BTreeMap::from([
                        (origin.into(), origin_path),
                        ("external".into(), vec!["external".into(), "root".into()]),
                    ]),
                },
            )]),
        }
    }
    let root = reference("root", &[], 1);
    let app = reference("app", &["root"], 2);
    let inner = reference("inner", &["app", "root"], 3);
    let sibling = reference("sibling", &["root"], 4);
    let registry = vec![root, app, inner, sibling];
    for local in &registry {
        assert_eq!(resolve(&local.context.root, &registry), *local);
        let mut reversed = registry.clone();
        reversed.reverse();
        assert_eq!(resolve(&local.context.root, &reversed), *local);
    }
    assert!(resolve("external", &registry).pins.is_empty());
    assert!(resolve("unrelated", &registry).pins.is_empty());
}

#[test]
fn reference_origin_and_target_cannot_claim_inconsistent_shared_ancestors() {
    let (_, _, source) = fixture();
    let mut set = resolve("inner", &[source]);
    set.ownership.clear();
    set.anchors.clear();
    set.presentation.insert(
        "leaf".into(),
        PinPresentation {
            path: ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
            groups: vec![],
            reference: Some(PinReference {
                origin: "inner".into(),
                target: "external".into(),
            }),
            supplied: BTreeMap::from([
                (
                    "inner".into(),
                    ["inner", "app", "root"].map(String::from).to_vec(),
                ),
                (
                    "external".into(),
                    ["external", "app", "different-root"]
                        .map(String::from)
                        .to_vec(),
                ),
            ]),
        },
    );
    let error = validate_constraint_registry(
        &set.context.model_id,
        &[set.clone()],
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.to_string().contains("shared supplied"));
    set.presentation
        .get_mut("leaf")
        .unwrap()
        .supplied
        .get_mut("external")
        .unwrap()[2] = "root".into();
    validate_constraint_registry(
        &set.context.model_id,
        std::slice::from_ref(&set),
        &AtomicBool::new(false),
    )
    .unwrap();
}
