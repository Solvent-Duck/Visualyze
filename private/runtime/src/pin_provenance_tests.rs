mod pin_provenance_tests {
    use super::*;
    use layout::{
        composed::{
            CanonicalPins, ComposedRequest, CompositionContext,
            persistence::{self as state_file, PersistedComposedView},
            solve_composed,
        },
        projection::DisplayGroup,
    };

    fn grouped_projection() -> (
        LocalTestFile,
        LayoutInput,
        ViewState,
        DesktopProjection,
        DisplayGroup,
    ) {
        let file = LocalTestFile::new();
        grouped_code_revision(&file.0, 1, 4, true);
        let input = LayoutInput::bundle(file.0.clone());
        let group = DisplayGroup {
            id: "view:pin-ancestry".into(),
            parent: "package".into(),
            members: vec!["file:000".into(), "file:001".into()],
            suggested: false,
        };
        let mut view = ViewState::default();
        view.semantic_zoom = false;
        view.frame_lens = Some(SemanticLens::Code);
        view.enter("package");
        view.group_overrides.push(group.clone());
        view.set_expanded(group.id.clone(), true);
        view.set_expanded("file:000", true);
        let projection = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Composed,
        );
        (file, input, view, projection, group)
    }

    fn constraints(
        projection: &DesktopProjection,
        view: &ViewState,
        owner: &str,
        object: &str,
        position: Point,
    ) -> ComposedConstraints {
        let mut constraints = ComposedConstraints::empty(CompositionContext {
            model_id: projection.model_id.clone(),
            root: projection.root.clone(),
            lens: projection.lens,
            policy: view.policy,
        });
        constraints
            .pins
            .insert(owner.into(), BTreeMap::from([(object.into(), position)]));
        constraints
    }

    fn retained_suggestion() -> (LocalTestFile, LayoutInput, ViewState, DisplayGroup) {
        let file = LocalTestFile::new();
        grouped_code_revision(&file.0, 1, 130, true);
        let input = LayoutInput::bundle(file.0.clone());
        let mut view = ViewState::default();
        view.semantic_zoom = false;
        view.frame_lens = Some(SemanticLens::Code);
        view.enter("package");
        // Seeded, not computed: see `seeded_suggestion`.
        view.protect_groups(&[seeded_suggestion(
            "package",
            &["file:000", "file:001", "file:002", "file:003"],
        )])
        .unwrap();
        let projection = input.project(&view, &AtomicBool::new(false)).unwrap();
        let group = projection
            .suggested_groups
            .iter()
            .find(|group| group.members.iter().any(|member| member == "file:000"))
            .unwrap()
            .clone();
        assert!(group.suggested);
        assert!(
            group.members.len() >= 3,
            "regression needs a group that can shrink"
        );
        view.remember_groups(&projection);
        view.enter("file:000");
        (file, input, view, group)
    }

    fn change_suggested_neighbors(
        file: &LocalTestFile,
        changed: &BTreeSet<String>,
        reparent: bool,
    ) {
        use visualyze_model::provider::*;
        let mut manifest = None;
        let mut records = Vec::new();
        let deleted: BTreeSet<_> = changed
            .iter()
            .flat_map(|id| [id.clone(), id.replacen("file:", "function:", 1)])
            .collect();
        for line in std::fs::read_to_string(&file.0).unwrap().lines() {
            match serde_json::from_str::<BundleRecord>(line).unwrap() {
                BundleRecord::Manifest(mut value) => {
                    value.revision += 1;
                    manifest = Some(value);
                }
                BundleRecord::Complete(_) => (),
                BundleRecord::Entity(record)
                    if !reparent && deleted.contains(&record.summary.id) => {}
                BundleRecord::Relation(record)
                    if !reparent
                        && (deleted.contains(&record.source)
                            || deleted.contains(&record.target)) => {}
                BundleRecord::Relation(mut record)
                    if reparent
                        && record.kind == RelationKind::OwnsCode
                        && changed.contains(&record.target) =>
                {
                    record.source = "repo".into();
                    records.push(BundleRecord::Relation(record));
                }
                record => records.push(record),
            }
        }
        let staged = LocalTestFile::new();
        let mut writer =
            BundleWriter::create(&staged.0, manifest.unwrap(), Limits::default()).unwrap();
        for record in records {
            writer.push(&record).unwrap();
        }
        writer.finish().unwrap();
        file.replace(&std::fs::read(&staged.0).unwrap());
    }

    #[test]
    fn first_pin_uses_pruned_or_dissolved_suggestions_after_neighbor_revision_changes() {
        for reparent in [false, true] {
            for survivors in [2, 1] {
                let (file, input, view, group) = retained_suggestion();
                let mut surviving = vec!["file:000".to_owned()];
                surviving.extend(
                    group
                        .members
                        .iter()
                        .filter(|id| id.as_str() != "file:000")
                        .take(survivors - 1)
                        .cloned(),
                );
                surviving.sort();
                let changed: BTreeSet<_> = group
                    .members
                    .iter()
                    .filter(|id| !surviving.contains(id))
                    .cloned()
                    .collect();
                change_suggested_neighbors(&file, &changed, reparent);
                let input = input.reload_candidate().unwrap();
                let projection = input.project(&view, &AtomicBool::new(false)).unwrap();
                assert_eq!(projection.revision, 2);
                assert_eq!(projection.root, "file:000");
                assert!(projection.nodes.iter().all(|node| node.id != group.id));
                assert_eq!(
                    view.retained_suggestions()
                        .iter()
                        .find(|old| old.id == group.id)
                        .unwrap(),
                    &group,
                    "the test must retain the stale suggestion before its first pin"
                );
                let normalized = projection
                    .group_catalog
                    .iter()
                    .find(|current| current.id == group.id);
                if survivors == 2 {
                    assert_eq!(normalized.unwrap().members, surviving);
                } else {
                    assert!(normalized.is_none(), "a singleton suggestion must dissolve");
                }
                let mut owned = constraints(
                    &projection,
                    &view,
                    "file:000",
                    "function:000",
                    Point::new(60, 40, 60),
                );
                verify_constraint_ownership(
                    &input,
                    &projection,
                    &mut owned,
                    &AtomicBool::new(false),
                )
                .unwrap();
                assert_eq!(
                    owned.ownership["function:000"],
                    vec!["function:000", "file:000", "package", "repo", "root"]
                );
                assert!(owned.anchors.is_empty());
                if survivors == 2 {
                    let metadata = &owned.presentation["function:000"];
                    assert_eq!(metadata.groups, vec![normalized.unwrap().clone()]);
                    assert_eq!(
                        metadata.path,
                        vec![
                            "function:000",
                            "file:000",
                            &group.id,
                            "package",
                            "repo",
                            "root"
                        ]
                    );
                    assert!(changed.iter().all(|id| !metadata.supplied.contains_key(id)));
                } else {
                    assert!(
                        owned.presentation.is_empty(),
                        "the dissolved group must not become protected"
                    );
                }
            }
        }
    }

    #[test]
    fn first_pin_uses_manual_correction_instead_of_overlapping_retained_suggestion() {
        let (_file, input, mut view, group) = retained_suggestion();
        let other = group
            .members
            .iter()
            .find(|id| id.as_str() != "file:000")
            .unwrap()
            .clone();
        let mut correction = DisplayGroup {
            id: "view:corrected-before-pin".into(),
            parent: "package".into(),
            members: vec!["file:000".into(), other],
            suggested: false,
        };
        correction.members.sort();
        view.group_overrides.push(correction.clone());
        let projection = input.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(projection.root, "file:000");
        assert!(
            projection
                .suggested_groups
                .iter()
                .all(|current| current.id != correction.id)
        );
        assert!(view.retained_suggestions().iter().any(|old| old == &group));
        let remainder = projection
            .group_catalog
            .iter()
            .find(|current| current.id == group.id);
        let mut expected: Vec<_> = group
            .members
            .iter()
            .filter(|id| !correction.members.contains(id))
            .cloned()
            .collect();
        expected.sort();
        if expected.len() >= 2 {
            assert_eq!(remainder.unwrap().members, expected);
        } else {
            assert!(remainder.is_none());
        }
        let mut owned = constraints(
            &projection,
            &view,
            "file:000",
            "function:000",
            Point::new(60, 40, 60),
        );
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();
        let metadata = &owned.presentation["function:000"];
        assert_eq!(metadata.groups, vec![correction.clone()]);
        assert_eq!(
            metadata.path,
            vec![
                "function:000",
                "file:000",
                &correction.id,
                "package",
                "repo",
                "root"
            ]
        );
        assert!(!metadata.path.contains(&group.id));
        assert_eq!(
            owned.ownership["function:000"],
            vec!["function:000", "file:000", "package", "repo", "root"]
        );
    }

    #[test]
    fn canonical_only_descendant_pin_captures_group_ancestry_without_an_anchor() {
        let (_file, input, view, projection, group) = grouped_projection();
        let mut owned = constraints(
            &projection,
            &view,
            "file:000",
            "function:000",
            Point::new(60, 40, 60),
        );
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();

        assert!(
            owned.anchors.is_empty(),
            "provenance must not fabricate an anchor"
        );
        assert_eq!(
            owned.ownership["function:000"],
            vec!["function:000", "file:000", "package", "repo", "root"]
        );
        let metadata = &owned.presentation["function:000"];
        assert_eq!(metadata.groups, vec![group.clone()]);
        assert_eq!(
            metadata.path,
            vec![
                "function:000",
                "file:000",
                &group.id,
                "package",
                "repo",
                "root"
            ]
        );
        assert_eq!(
            metadata
                .supplied
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["file:000", "file:001", "package"]
        );
        assert_eq!(owned.active(&projection).unwrap().pins, owned.pins);

        let mut inside = owned.context.clone();
        inside.root = group.id.clone();
        let registry = BTreeMap::from([(
            (owned.context.lens, owned.context.root.clone()),
            owned.clone(),
        )]);
        let inherited =
            ComposedConstraints::resolve(&inside, &registry, &AtomicBool::new(false)).unwrap();
        assert_eq!(inherited.pins, owned.pins);
        assert_eq!(inherited.presentation, owned.presentation);
        assert!(inherited.anchors.is_empty());
        assert_eq!(registry.values().next().unwrap(), &owned);
    }

    #[test]
    fn pin_created_inside_a_file_retains_its_offscreen_group_coordinate_ancestry() {
        let (_file, input, mut view, previous, group) = grouped_projection();
        view.remember_groups(&previous);
        view.enter("file:000");
        let projection = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Composed,
        );
        assert!(projection.nodes.iter().all(|node| node.id != group.id));
        assert!(
            projection
                .suggested_groups
                .iter()
                .all(|current| current.id != group.id)
        );
        let mut owned = constraints(
            &projection,
            &view,
            "file:000",
            "function:000",
            Point::new(60, 40, 60),
        );
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();
        assert!(owned.anchors.is_empty());
        assert_eq!(
            owned.presentation["function:000"].groups,
            vec![group.clone()]
        );
        assert_eq!(
            owned.presentation["function:000"].path,
            vec![
                "function:000",
                "file:000",
                &group.id,
                "package",
                "repo",
                "root"
            ]
        );
        assert_eq!(
            owned.ownership["function:000"],
            vec!["function:000", "file:000", "package", "repo", "root"]
        );
        let registry = BTreeMap::from([(
            (owned.context.lens, owned.context.root.clone()),
            owned.clone(),
        )]);
        let mut parent = owned.context.clone();
        parent.root = group.id;
        let inherited =
            ComposedConstraints::resolve(&parent, &registry, &AtomicBool::new(false)).unwrap();
        assert_eq!(inherited.pins, owned.pins);
        assert_eq!(inherited.presentation, owned.presentation);
        assert!(inherited.anchors.is_empty());
    }

    #[test]
    fn reordered_group_members_do_not_change_pin_identity_or_supplied_proof() {
        let (_file, input, view, mut projection, group) = grouped_projection();
        let mut owned = constraints(
            &projection,
            &view,
            &group.id,
            "file:000",
            Point::new(60, 40, 60),
        );
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();
        let before = owned.clone();
        projection
            .graph
            .suggested_groups
            .iter_mut()
            .find(|current| current.id == group.id)
            .unwrap()
            .members
            .reverse();
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(owned, before);
    }

    #[test]
    fn shared_reference_identity_is_verified_but_never_inherited_into_another_origin() {
        let file = LocalTestFile::new();
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let input = LayoutInput::bundle(file.0.clone());
        let mut view = ViewState::default();
        view.semantic_zoom = false;
        view.frame_lens = Some(SemanticLens::Logical);
        view.enter("service:orders");
        let origin = input.project(&view, &AtomicBool::new(false)).unwrap();
        let reference = origin
            .references
            .iter()
            .find(|(_, target)| target.as_str() == "datastore:orders")
            .expect("orders service must reference the visible database ancestor")
            .0
            .clone();
        let mut owned = constraints(
            &origin,
            &view,
            &origin.root,
            &reference,
            Point::new(60, 40, 60),
        );
        verify_constraint_ownership(&input, &origin, &mut owned, &AtomicBool::new(false)).unwrap();
        let before = owned.clone();
        view.enter("service:worker");
        let foreign = input.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(foreign.references[&reference], "datastore:orders");
        verify_constraint_ownership(&input, &foreign, &mut owned, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            owned, before,
            "visiting another origin must not rebind the reference"
        );

        let registry = BTreeMap::from([(
            (owned.context.lens, owned.context.root.clone()),
            owned.clone(),
        )]);
        let mut target_context = owned.context.clone();
        for root in [
            foreign.root.as_str(),
            "datastore:orders",
            "table:orders",
            "scope:orders",
        ] {
            target_context.root = root.into();
            let effective =
                ComposedConstraints::resolve(&target_context, &registry, &AtomicBool::new(false))
                    .unwrap();
            assert!(effective.pins.is_empty(), "reference leaked into {root}");
            assert!(effective.presentation.is_empty());
            assert!(effective.anchors.is_empty());
        }
        assert_eq!(
            ComposedConstraints::resolve(&owned.context, &registry, &AtomicBool::new(false))
                .unwrap(),
            owned
        );
    }

    #[test]
    fn directly_pinned_group_round_trips_without_claiming_supplied_ownership() {
        let (_file, input, mut view, _expanded, group) = grouped_projection();
        view.set_expanded(group.id.clone(), false);
        let projection = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Composed,
        );
        let accepted = solve_composed(ComposedRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins: &CanonicalPins::new(),
            work_budget: 32,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        let point = accepted
            .assembly
            .frame(&projection.root)
            .unwrap()
            .solution()
            .boxes[&group.id]
            .bounds
            .center();
        let mut owned = constraints(&projection, &view, &projection.root, &group.id, point);
        verify_constraint_ownership(&input, &projection, &mut owned, &AtomicBool::new(false))
            .unwrap();
        assert!(owned.ownership.is_empty());
        assert_eq!(owned.presentation[&group.id].groups, vec![group.clone()]);
        assert_eq!(
            owned.presentation[&group.id].path,
            vec![group.id.as_str(), "package", "repo", "root"]
        );
        let mut saved = PersistedComposedView {
            version: state_file::STATE_VERSION,
            model_id: projection.model_id.clone(),
            revision: projection.revision,
            context: owned.context.clone(),
            view,
            canonical_pins: owned.pins.clone(),
            anchor_pins: vec![],
            constraint_sets: vec![owned.persisted()],
            dismissed_proposals: vec![],
            accepted,
        };
        saved
            .validate(&projection, &AtomicBool::new(false))
            .unwrap();
        let directory = LocalTestDirectory::new();
        let path = composed_state_path(&directory.0, &projection.model_id);
        state_file::save(&path, &saved, &AtomicBool::new(false)).unwrap();
        let mut restored = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        restored
            .validate(&projection, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(restored.constraint_sets, saved.constraint_sets);
        let mut restored_owned: ComposedConstraints = restored.constraint_sets[0].clone().into();
        verify_constraint_ownership(
            &input,
            &projection,
            &mut restored_owned,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(restored_owned, owned);
        assert_eq!(restored_owned.active(&projection).unwrap().pins, owned.pins);
    }
}
