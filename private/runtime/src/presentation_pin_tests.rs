mod presentation_pin_tests {
    use super::*;
    use layout::{
        assembly::AnchorPin,
        composed::{ComposedSolution, persistence as state_file},
        projection::DisplayGroup,
    };

    fn wait_until(app: &mut App, ready: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.update();
            if ready(app) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "presentation-pin worker timed out: {}",
                app.world().resource::<Viewer>().message
            );
            std::thread::yield_now();
        }
    }

    fn complete(app: &mut App, action: ViewerAction) {
        let ticket = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(action)
            .unwrap();
        wait_until(app, |app| {
            let status = app.world().resource::<LayoutStatus>();
            status.action_finished(ticket) && !status.loading
        });
    }

    fn cold(input: LayoutInput, directory: &std::path::Path, mut view: ViewState) -> App {
        view.semantic_zoom = false;
        let mut viewer = Viewer {
            view,
            storage: Some(directory.into()),
            ..default()
        };
        start(&input, LayoutEngine::Composed, &mut viewer, false);
        let mut app = App::new();
        app.insert_resource(input)
            .insert_resource(LayoutEngine::Composed)
            .insert_resource(viewer)
            .init_resource::<LayoutStatus>()
            .init_resource::<ViewerActions>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, (process_actions, poll, navigation_status).chain());
        wait_until(&mut app, |app| {
            app.world().resource::<Viewer>().job.is_none()
        });
        ready(&app);
        app
    }

    fn ready(app: &App) {
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        assert!(app.world().resource::<Viewer>().accepted.is_some());
        assert_eq!(
            status.save_failures,
            0,
            "{}",
            app.world().resource::<Viewer>().message
        );
    }

    fn accepted(app: &App) -> &NativeLayout {
        &app.world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
    }

    fn canonical(app: &App) -> &ComposedSolution {
        accepted(app).composed_solution().unwrap()
    }

    fn projection(app: &App) -> &DesktopProjection {
        &app.world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0
    }

    fn path(app: &App, directory: &LocalTestDirectory) -> std::path::PathBuf {
        composed_state_path(&directory.0, &accepted(app).model_id)
    }

    fn evict_geometry(app: &mut App) {
        let mut viewer = app.world_mut().resource_mut::<Viewer>();
        viewer.cache.clear();
        viewer.cache_order.clear();
        viewer.canonical_cache = Arc::new(layout::composed::CanonicalPlacementCache::default());
    }

    fn validate_active(app: &App) {
        let LayoutData::Composed { anchors, .. } = &accepted(app).data else {
            panic!("expected composed geometry")
        };
        assert!(!anchors.is_empty());
        for anchor in anchors {
            anchor.validate(&canonical(app).assembly).unwrap();
        }
    }

    fn grouped_app(suggested: bool) -> (LocalTestFile, LocalTestDirectory, App, DisplayGroup) {
        let file = LocalTestFile::new();
        grouped_code_revision(&file.0, 1, if suggested { 8 } else { 4 }, true);
        let directory = LocalTestDirectory::new();
        let mut view = ViewState::default();
        view.frame_lens = Some(SemanticLens::Code);
        view.enter("package");
        if suggested {
            // Seeded, not computed: see `seeded_suggestion`.
            view.protect_groups(&[seeded_suggestion(
                "package",
                &["file:000", "file:001", "file:002", "file:003"],
            )])
            .unwrap();
        }
        let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0, view);
        if !suggested {
            complete(
                &mut app,
                ViewerAction::SetGroup {
                    id: "view:manual-pins".into(),
                    parent: "package".into(),
                    members: vec!["file:000".into(), "file:001".into()],
                },
            );
            ready(&app);
        }
        let group = projection(&app)
            .suggested_groups
            .iter()
            .find(|group| group.members.iter().any(|id| id == "file:000"))
            .unwrap()
            .clone();
        assert_eq!(group.suggested, suggested);
        complete(&mut app, ViewerAction::Expand(group.id.clone()));
        ready(&app);
        (file, directory, app, group)
    }

    fn pin_member(app: &mut App, group: &DisplayGroup) -> (ComposedConstraints, AnchorPin) {
        let before = canonical(app).assembly.clone();
        complete(app, ViewerAction::Pin("file:000".into()));
        ready(app);
        assert_eq!(
            canonical(app).assembly,
            before,
            "pinning rearranged the scene"
        );
        let owned = accepted(app).owned_constraints().unwrap();
        assert_eq!(owned.presentation["file:000"].groups, vec![group.clone()]);
        assert_eq!(
            owned.presentation["file:000"].path,
            vec!["file:000", &group.id, "package", "repo", "root"]
        );
        assert_eq!(
            owned.ownership["file:000"],
            vec!["file:000", "package", "repo", "root"]
        );
        let pin = owned
            .anchors
            .iter()
            .find(|pin| pin.object_id() == "file:000")
            .unwrap()
            .clone();
        pin.validate(&canonical(app).assembly).unwrap();
        (owned, pin)
    }

    #[test]
    fn manual_and_real_suggested_group_member_pins_survive_navigation_eviction_and_restart() {
        for suggested in [false, true] {
            let (file, directory, mut app, group) = grouped_app(suggested);
            let (owned, pin) = pin_member(&mut app, &group);
            let siblings = canonical(&app)
                .assembly
                .frame("package")
                .unwrap()
                .solution()
                .boxes
                .clone();
            let original_group = canonical(&app)
                .assembly
                .frame(&group.id)
                .unwrap()
                .solution()
                .boxes
                .clone();
            complete(&mut app, ViewerAction::Collapse(group.id.clone()));
            ready(&app);
            assert!(canonical(&app).assembly.frame(&group.id).is_err());
            assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
            complete(&mut app, ViewerAction::Expand(group.id.clone()));
            ready(&app);
            pin.validate(&canonical(&app).assembly).unwrap();
            assert_eq!(
                canonical(&app)
                    .assembly
                    .frame(&group.id)
                    .unwrap()
                    .solution()
                    .boxes,
                original_group
            );

            complete(&mut app, ViewerAction::Enter(group.id.clone()));
            ready(&app);
            assert_eq!(canonical(&app).assembly.root(), group.id);
            assert!(is_pinned(app.world().resource::<Viewer>(), "file:000"));
            assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
            assert_eq!(
                canonical(&app)
                    .assembly
                    .frame(&group.id)
                    .unwrap()
                    .solution()
                    .boxes["file:000"]
                    .bounds
                    .center(),
                pin.local_point()
            );
            validate_active(&app);
            if suggested {
                // Restart while the only focused identity is a display group,
                // after its ordinary recent-definition cache has been evicted.
                let saved_path = path(&app, &directory);
                let mut wire: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
                wire["view"]["retained_groups"] = serde_json::json!([]);
                std::fs::write(&saved_path, serde_json::to_vec(&wire).unwrap()).unwrap();
                drop(app);
                app = cold(
                    LayoutInput::bundle(file.0.clone()),
                    &directory.0,
                    ViewState::default(),
                );
                assert_eq!(canonical(&app).assembly.root(), group.id);
                assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
                assert_eq!(
                    canonical(&app)
                        .assembly
                        .frame(&group.id)
                        .unwrap()
                        .solution()
                        .boxes["file:000"]
                        .bounds
                        .center(),
                    pin.local_point()
                );
                validate_active(&app);
            }
            complete(&mut app, ViewerAction::Back);
            ready(&app);
            pin.validate(&canonical(&app).assembly).unwrap();
            assert_eq!(
                canonical(&app)
                    .assembly
                    .frame("package")
                    .unwrap()
                    .solution()
                    .boxes,
                siblings
            );

            // Remove the recent suggestion cache as it would be after eviction;
            // the independently persisted pin registry is the only group proof.
            if suggested {
                let mut viewer = app.world_mut().resource_mut::<Viewer>();
                let mut wire = serde_json::to_value(&viewer.view).unwrap();
                wire["retained_groups"] = serde_json::json!([]);
                viewer.view = serde_json::from_value(wire).unwrap();
                assert!(viewer.view.retained_suggestions().is_empty());
            }
            evict_geometry(&mut app);
            complete(&mut app, ViewerAction::Collapse(group.id.clone()));
            ready(&app);
            let saved_path = path(&app, &directory);
            let stored = state_file::load(&saved_path, &AtomicBool::new(false)).unwrap();
            assert_eq!(stored.version, 6);
            assert!(
                stored
                    .constraint_sets
                    .iter()
                    .any(|set| set.presentation == owned.presentation)
            );
            drop(app);

            let mut app = cold(
                LayoutInput::bundle(file.0.clone()),
                &directory.0,
                ViewState::default(),
            );
            evict_geometry(&mut app);
            complete(&mut app, ViewerAction::Expand(group.id.clone()));
            ready(&app);
            pin.validate(&canonical(&app).assembly).unwrap();
            assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
            assert_eq!(
                canonical(&app)
                    .assembly
                    .frame(&group.id)
                    .unwrap()
                    .solution()
                    .boxes["file:000"]
                    .bounds
                    .center(),
                pin.local_point()
            );
            assert_eq!(
                canonical(&app)
                    .assembly
                    .frame("package")
                    .unwrap()
                    .solution()
                    .boxes,
                siblings
            );
            assert_eq!(app.world().resource::<LayoutStatus>().save_failures, 0);
        }
    }

    #[test]
    fn pins_created_inside_grouped_children_keep_enclosing_frames_after_restart() {
        for suggested in [false, true] {
            let (file, directory, mut app, group) = grouped_app(suggested);
            complete(&mut app, ViewerAction::Enter(group.id.clone()));
            ready(&app);
            complete(&mut app, ViewerAction::Enter("file:000".into()));
            ready(&app);
            assert!(
                projection(&app)
                    .suggested_groups
                    .iter()
                    .all(|g| g.id != group.id)
            );
            complete(&mut app, ViewerAction::Pin("function:000".into()));
            ready(&app);
            let owned = accepted(&app).owned_constraints().unwrap();
            assert_eq!(
                owned.presentation["function:000"].groups,
                vec![group.clone()]
            );
            let pin = owned.anchors[0].clone();
            assert_eq!(pin.anchor(), "file:000");
            let saved_path = path(&app, &directory);
            if suggested {
                let mut wire: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
                wire["view"]["retained_groups"] = serde_json::json!([]);
                std::fs::write(&saved_path, serde_json::to_vec(&wire).unwrap()).unwrap();
            }
            drop(app);
            let mut app = cold(
                LayoutInput::bundle(file.0.clone()),
                &directory.0,
                ViewState::default(),
            );
            assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
            evict_geometry(&mut app);
            complete(&mut app, ViewerAction::Back);
            ready(&app);
            assert_eq!(projection(&app).root, group.id);
            assert!(is_pinned(app.world().resource::<Viewer>(), "function:000"));
            complete(&mut app, ViewerAction::Expand("file:000".into()));
            ready(&app);
            pin.validate(&canonical(&app).assembly).unwrap();
            assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
            complete(&mut app, ViewerAction::OpenPinOrigin("function:000".into()));
            ready(&app);
            assert_eq!(projection(&app).root, "file:000");
            assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
        }
    }

    #[test]
    #[ignore = "requires the locally produced pinned Bevy syntax-v2 bundle"]
    fn production_grouped_descendant_pin_survives_restart_and_parent_navigation() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl");
        let directory = LocalTestDirectory::new();
        let input = LayoutInput::bundle(path);
        let mut view = ViewState::default();
        view.frame_lens = Some(SemanticLens::Code);
        view.enter("package:d1c2531f3a0a9b57");
        let mut app = cold(input.clone(), &directory.0, view);
        let mut selected = None;
        for group in projection(&app)
            .suggested_groups
            .iter()
            .filter(|g| g.suggested)
        {
            for member in &group.members {
                let mut inside = app.world().resource::<Viewer>().view.clone();
                inside.enter(member.clone());
                let p = input.project(&inside, &AtomicBool::new(false)).unwrap();
                if let Some(child) = p.nodes.iter().find(|n| {
                    n.id != p.root
                        && !p.references.contains_key(&n.id)
                        && !p.suggested_groups.iter().any(|g| g.id == n.id)
                }) {
                    selected = Some((group.clone(), member.clone(), child.id.clone()));
                    break;
                }
            }
            if selected.is_some() {
                break;
            }
        }
        let (group, member, child) = selected.expect(
            "production package must have a grouped source file with supplied declarations",
        );
        complete(&mut app, ViewerAction::Enter(group.id.clone()));
        ready(&app);
        complete(&mut app, ViewerAction::Enter(member.clone()));
        ready(&app);
        complete(&mut app, ViewerAction::Pin(child.clone()));
        ready(&app);
        let owned = accepted(&app).owned_constraints().unwrap();
        let pin = owned
            .anchors
            .iter()
            .find(|pin| pin.object_id() == child)
            .unwrap()
            .clone();
        assert!(owned.presentation[&child].groups.contains(&group));
        let saved_path = self::path(&app, &directory);
        let mut wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
        wire["view"]["retained_groups"] = serde_json::json!([]);
        std::fs::write(&saved_path, serde_json::to_vec(&wire).unwrap()).unwrap();
        drop(app);
        let mut app = cold(input, &directory.0, ViewState::default());
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
        evict_geometry(&mut app);
        complete(&mut app, ViewerAction::Back);
        ready(&app);
        assert_eq!(projection(&app).root, group.id);
        assert!(is_pinned(app.world().resource::<Viewer>(), &child));
        complete(&mut app, ViewerAction::Expand(member.clone()));
        ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
        complete(&mut app, ViewerAction::OpenPinOrigin(child));
        ready(&app);
        assert_eq!(projection(&app).root, member);
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
    }

    fn assert_rejected_unchanged(
        app: &mut App,
        directory: &LocalTestDirectory,
        action: ViewerAction,
    ) {
        let old = accepted(app).clone();
        let registry = app
            .world()
            .resource::<Viewer>()
            .composed_constraints
            .clone();
        let view = serde_json::to_value(&app.world().resource::<Viewer>().accepted_view).unwrap();
        let file = path(app, directory);
        let bytes = std::fs::read(&file).unwrap();
        complete(app, action);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_some()
        );
        assert_eq!(accepted(app), &old);
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints,
            registry
        );
        assert_eq!(
            serde_json::to_value(&app.world().resource::<Viewer>().view).unwrap(),
            view
        );
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }

    #[test]
    fn protected_manual_and_suggested_groups_reject_explicit_removal_or_correction_transactionally()
    {
        for suggested in [false, true] {
            let (_file, directory, mut app, group) = grouped_app(suggested);
            pin_member(&mut app, &group);
            assert_rejected_unchanged(
                &mut app,
                &directory,
                ViewerAction::RemoveGroup(group.id.clone()),
            );
            assert_rejected_unchanged(
                &mut app,
                &directory,
                ViewerAction::SetGroup {
                    id: group.id,
                    parent: group.parent,
                    members: vec!["file:000".into(), "file:002".into()],
                },
            );
        }
    }

    fn replace_bundle(
        file: &LocalTestFile,
        edit: impl FnOnce(&mut Vec<visualyze_model::provider::BundleRecord>),
    ) {
        use visualyze_model::provider::*;
        let mut manifest = None;
        let mut records = Vec::new();
        for line in std::fs::read_to_string(&file.0).unwrap().lines() {
            match serde_json::from_str::<BundleRecord>(line).unwrap() {
                BundleRecord::Manifest(value) => manifest = Some(value),
                BundleRecord::Complete(_) => (),
                record => records.push(record),
            }
        }
        edit(&mut records);
        let mut manifest = manifest.unwrap();
        manifest.revision += 1;
        let staged = LocalTestFile::new();
        let mut writer = BundleWriter::create(&staged.0, manifest, Limits::default()).unwrap();
        for record in records {
            writer.push(&record).unwrap();
        }
        writer.finish().unwrap();
        file.replace(&std::fs::read(&staged.0).unwrap());
    }

    #[test]
    fn new_pin_after_unpinned_group_shrinks_survives_following_actions_and_restart() {
        use visualyze_model::provider::BundleRecord;
        let (file, directory, mut app, group) = grouped_app(true);
        assert!(group.members.len() > 2);
        complete(&mut app, ViewerAction::Enter("file:000".into()));
        ready(&app);
        let removed: BTreeSet<_> = group
            .members
            .iter()
            .skip(2)
            .flat_map(|id| [id.clone(), id.replacen("file:", "function:", 1)])
            .collect();
        replace_bundle(&file, |records| {
            records.retain(|record| match record {
                BundleRecord::Entity(entity) => !removed.contains(&entity.summary.id),
                BundleRecord::Relation(relation) => {
                    !removed.contains(&relation.source) && !removed.contains(&relation.target)
                }
                _ => true,
            })
        });
        complete(&mut app, ViewerAction::Reload);
        ready(&app);
        assert_eq!(canonical(&app).revision, 2);
        complete(&mut app, ViewerAction::Pin("function:000".into()));
        ready(&app);
        let owned = accepted(&app).owned_constraints().unwrap();
        assert_eq!(
            owned.presentation["function:000"].groups[0].members,
            group.members[..2]
        );
        let pin = owned.anchors[0].clone();
        drop(app);
        let mut app = cold(
            LayoutInput::bundle(file.0.clone()),
            &directory.0,
            ViewState::default(),
        );
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
        complete(&mut app, ViewerAction::Jump(group.id));
        ready(&app);
        complete(&mut app, ViewerAction::Expand("file:000".into()));
        ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
    }

    #[test]
    fn removed_or_reparented_group_members_reject_reload_without_losing_the_pin() {
        use visualyze_model::provider::BundleRecord;
        for reparent in [false, true] {
            let (file, directory, mut app, group) = grouped_app(false);
            pin_member(&mut app, &group);
            replace_bundle(&file, |records| {
                if reparent {
                    for record in records {
                        if let BundleRecord::Relation(relation) = record
                            && relation.id == "owns:file:001"
                        {
                            relation.source = "repo".into();
                        }
                    }
                } else {
                    records.retain(|record| match record {
                        BundleRecord::Entity(entity) => {
                            !matches!(entity.summary.id.as_str(), "file:001" | "function:001")
                        }
                        BundleRecord::Relation(relation) => {
                            !matches!(relation.source.as_str(), "file:001" | "function:001")
                                && !matches!(relation.target.as_str(), "file:001" | "function:001")
                        }
                        _ => true,
                    });
                }
            });
            assert_rejected_unchanged(&mut app, &directory, ViewerAction::Reload);
            assert_eq!(canonical(&app).revision, 1);
            assert!(is_pinned(app.world().resource::<Viewer>(), "file:000"));
        }
    }

    #[test]
    fn presentation_pin_unpin_undo_and_prepublication_cancellation_are_exact() {
        let (_file, directory, mut app, group) = grouped_app(false);
        let (owned, pin) = pin_member(&mut app, &group);
        let before = accepted(&app).clone();
        complete(&mut app, ViewerAction::Unpin("file:000".into()));
        ready(&app);
        assert!(!is_pinned(app.world().resource::<Viewer>(), "file:000"));
        assert!(
            app.world()
                .resource::<Viewer>()
                .composed_constraints
                .is_empty()
        );
        complete(&mut app, ViewerAction::Undo);
        ready(&app);
        assert_eq!(accepted(&app), &before);
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
        pin.validate(&canonical(&app).assembly).unwrap();

        let registry = app
            .world()
            .resource::<Viewer>()
            .composed_constraints
            .clone();
        let saved_path = path(&app, &directory);
        let bytes = std::fs::read(&saved_path).unwrap();
        let input = app.world().resource::<LayoutInput>().clone();
        let signal = {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            assert!(
                apply_action(
                    ViewerAction::Pin("file:001".into()),
                    &input,
                    LayoutEngine::Composed,
                    &mut viewer
                )
                .unwrap()
            );
            viewer.job.as_ref().unwrap().cancel.clone()
        };
        // The cancellation action executes before poll, even if the real
        // worker has completed. No timing threshold determines the outcome.
        complete(&mut app, ViewerAction::Cancel);
        assert!(signal.load(Ordering::Relaxed));
        assert_eq!(accepted(&app), &before);
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints,
            registry
        );
        assert!(!is_pinned(app.world().resource::<Viewer>(), "file:001"));
        assert_eq!(std::fs::read(saved_path).unwrap(), bytes);
    }

    const OUTSIDE_TARGETS: usize = 24;

    fn reference_file(file: &LocalTestFile) {
        use visualyze_model::provider::*;
        let mut writer = BundleWriter::create(
            &file.0,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "durable-reference-pins".into(),
                revision: 1,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture],
                coverage: Coverage::Complete,
                scope_description: "Bounded reference-page and origin-isolation regression".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        let mut entities = BTreeMap::from([
            ("root".to_owned(), EntityKind::Scope),
            ("origin:a".to_owned(), EntityKind::Scope),
            ("origin:b".to_owned(), EntityKind::Scope),
            ("inside:a".to_owned(), EntityKind::Service),
            ("inside:b".to_owned(), EntityKind::Service),
        ]);
        // More outside targets than one layer's reference slots (24 less the
        // origin's own peers, ADR 0007), so the relationships need several pages.
        for index in 0..OUTSIDE_TARGETS {
            entities.insert(format!("outside:{index}"), EntityKind::Service);
        }
        for (id, kind) in entities {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        label: id.clone(),
                        id,
                        kind,
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: None,
                }))
                .unwrap();
        }
        let mut relations = BTreeMap::from([
            (
                "owns:origin:a".to_owned(),
                (
                    RelationKind::Contains,
                    "root".to_owned(),
                    "origin:a".to_owned(),
                ),
            ),
            (
                "owns:origin:b".to_owned(),
                (
                    RelationKind::Contains,
                    "root".to_owned(),
                    "origin:b".to_owned(),
                ),
            ),
            (
                "owns:inside:a".to_owned(),
                (
                    RelationKind::Contains,
                    "origin:a".to_owned(),
                    "inside:a".to_owned(),
                ),
            ),
            (
                "owns:inside:b".to_owned(),
                (
                    RelationKind::Contains,
                    "origin:b".to_owned(),
                    "inside:b".to_owned(),
                ),
            ),
        ]);
        for index in 0..OUTSIDE_TARGETS {
            relations.insert(
                format!("owns:outside:{index}"),
                (
                    RelationKind::Contains,
                    "root".into(),
                    format!("outside:{index}"),
                ),
            );
            relations.insert(
                format!("flow:a:{index}"),
                (
                    RelationKind::Requests,
                    "inside:a".into(),
                    format!("outside:{index}"),
                ),
            );
        }
        relations.insert(
            "flow:b:0".into(),
            (
                RelationKind::Requests,
                "inside:b".into(),
                "outside:0".into(),
            ),
        );
        for (id, (kind, source, target)) in relations {
            writer
                .push(&BundleRecord::Relation(RelationRecord {
                    id,
                    kind,
                    source,
                    target,
                    claim: Claim::declared(),
                    label: None,
                    aspect: None,
                }))
                .unwrap();
        }
        writer.finish().unwrap();
    }

    fn reference_app() -> (LocalTestFile, LocalTestDirectory, App, String) {
        let file = LocalTestFile::new();
        reference_file(&file);
        let directory = LocalTestDirectory::new();
        let mut view = ViewState::default();
        view.frame_lens = Some(SemanticLens::Logical);
        view.enter("origin:a");
        let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0, view);
        assert!(
            projection(&app)
                .relation_page
                .as_ref()
                .unwrap()
                .next
                .is_some()
        );
        let reference = projection(&app)
            .references
            .iter()
            .find(|(_, target)| *target == "outside:0")
            .unwrap()
            .0
            .clone();
        complete(&mut app, ViewerAction::Pin(reference.clone()));
        ready(&app);
        (file, directory, app, reference)
    }

    fn no_active_pins(app: &App) {
        let LayoutData::Composed { pins, anchors, .. } = &accepted(app).data else {
            panic!("expected composed geometry")
        };
        assert!(pins.is_empty());
        assert!(anchors.is_empty());
    }

    #[test]
    fn reference_pin_is_dormant_across_pages_and_stays_with_its_origin_after_restart() {
        let (file, directory, mut app, reference) = reference_app();
        let owned = accepted(&app).owned_constraints().unwrap();
        let metadata = &owned.presentation[&reference];
        let binding = metadata.reference.as_ref().unwrap();
        assert_eq!(binding.origin, "origin:a");
        assert_eq!(binding.target, "outside:0");
        assert_eq!(metadata.path, vec![reference.as_str(), "origin:a", "root"]);
        assert_eq!(metadata.supplied["outside:0"], vec!["outside:0", "root"]);
        assert!(!owned.ownership.contains_key(&reference));
        let pin = owned.anchors[0].clone();
        let inside = canonical(&app)
            .assembly
            .frame("origin:a")
            .unwrap()
            .solution()
            .boxes["inside:a"]
            .clone();
        complete(&mut app, ViewerAction::NextRelations);
        ready(&app);
        assert!(!projection(&app).references.contains_key(&reference));
        no_active_pins(&app);
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
        let saved_path = path(&app, &directory);
        let saved = state_file::load(&saved_path, &AtomicBool::new(false)).unwrap();
        assert!(saved.canonical_pins.is_empty());
        assert_eq!(saved.constraint_sets.len(), 1);
        assert_eq!(saved.constraint_sets[0].presentation, owned.presentation);
        drop(app);

        let mut app = cold(
            LayoutInput::bundle(file.0.clone()),
            &directory.0,
            ViewState::default(),
        );
        assert!(!projection(&app).references.contains_key(&reference));
        no_active_pins(&app);
        evict_geometry(&mut app);
        complete(&mut app, ViewerAction::PreviousRelations);
        ready(&app);
        assert_eq!(projection(&app).references[&reference], "outside:0");
        pin.validate(&canonical(&app).assembly).unwrap();
        // A full page of references (ADR 0007) is large enough that a cold
        // re-solve, without the continuity of the original session, may shift
        // unpinned boxes; the pinned reference is exact (`pin.validate`), and the
        // origin's own content must keep its identity, parent and size.
        let restored = canonical(&app)
            .assembly
            .frame("origin:a")
            .unwrap()
            .solution()
            .boxes["inside:a"]
            .clone();
        assert_eq!(restored.parent, inside.parent);
        assert_eq!(restored.semantic_ids, inside.semantic_ids);
        assert_eq!(restored.bounds.size(), inside.bounds.size());

        complete(&mut app, ViewerAction::Enter(reference.clone()));
        ready(&app);
        assert_eq!(canonical(&app).assembly.root(), "outside:0");
        assert!(!is_pinned(app.world().resource::<Viewer>(), "outside:0"));
        no_active_pins(&app);
        complete(&mut app, ViewerAction::Jump("origin:b".into()));
        ready(&app);
        assert_eq!(projection(&app).references[&reference], "outside:0");
        assert!(!is_pinned(app.world().resource::<Viewer>(), &reference));
        no_active_pins(&app);
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
        drop(app);

        let mut app = cold(
            LayoutInput::bundle(file.0.clone()),
            &directory.0,
            ViewState::default(),
        );
        assert_eq!(canonical(&app).assembly.root(), "origin:b");
        assert!(!is_pinned(app.world().resource::<Viewer>(), &reference));
        no_active_pins(&app);
        complete(&mut app, ViewerAction::Jump("origin:a".into()));
        ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
        assert_eq!(accepted(&app).owned_constraints().unwrap(), owned);
    }

    #[test]
    fn deleted_or_reparented_reference_targets_reject_reload_even_on_another_page() {
        use visualyze_model::provider::BundleRecord;
        for reparent in [false, true] {
            let (file, directory, mut app, reference) = reference_app();
            complete(&mut app, ViewerAction::NextRelations);
            ready(&app);
            assert!(!projection(&app).references.contains_key(&reference));
            replace_bundle(&file, |records| {
                if reparent {
                    for record in records {
                        if let BundleRecord::Relation(relation) = record
                            && relation.id == "owns:outside:0"
                        {
                            relation.source = "origin:b".into();
                        }
                    }
                } else {
                    records.retain(|record| match record {
                        BundleRecord::Entity(entity) => entity.summary.id != "outside:0",
                        BundleRecord::Relation(relation) => {
                            relation.source != "outside:0" && relation.target != "outside:0"
                        }
                        _ => true,
                    });
                }
            });
            assert_rejected_unchanged(&mut app, &directory, ViewerAction::Reload);
            assert_eq!(canonical(&app).revision, 1);
        }
    }
}
