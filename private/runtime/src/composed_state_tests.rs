mod composed_state_tests {
    use super::*;
    use layout::composed::{
        CanonicalPins, ComposedRequest, CompositionContext,
        persistence::{self as state_file, PersistedComposedView},
        solve_composed,
    };

    fn cold(input: LayoutInput, directory: &std::path::Path) -> App {
        let mut viewer = Viewer {
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
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.update();
            if app.world().resource::<Viewer>().job.is_none() {
                return app;
            }
            assert!(Instant::now() < deadline, "composed startup timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn nested_saved(file: &LocalTestFile) -> (LayoutInput, PersistedComposedView) {
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let input = LayoutInput::bundle(file.0.clone());
        let mut view = ViewState::default();
        view.semantic_zoom = false;
        view.frame_lens = Some(SemanticLens::Logical);
        view.enter("scope:orders");
        view.set_expanded("datastore:orders", true);
        let p = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Composed,
        );
        let accepted = solve_composed(ComposedRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins: &CanonicalPins::new(),
            work_budget: 192,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        let root_pin = accepted.assembly.frame(&p.root).unwrap().solution().boxes["service:orders"]
            .bounds
            .center();
        let table_pin = accepted
            .assembly
            .frame("datastore:orders")
            .unwrap()
            .solution()
            .boxes["table:customers"]
            .bounds
            .center();
        let anchors = vec![
            layout::assembly::AnchorPin::capture(
                &accepted.assembly,
                "datastore:orders",
                &p.root,
                "table:customers",
            )
            .unwrap(),
        ];
        let context = CompositionContext {
            model_id: p.model_id.clone(),
            root: p.root.clone(),
            lens: p.lens,
            policy: view.policy,
        };
        let canonical_pins = BTreeMap::from([
            (
                p.root.clone(),
                BTreeMap::from([("service:orders".into(), root_pin)]),
            ),
            (
                "datastore:orders".into(),
                BTreeMap::from([("table:customers".into(), table_pin)]),
            ),
        ]);
        let native = NativeLayout::composed(
            accepted.clone(),
            canonical_pins.clone(),
            anchors.clone(),
            context.clone(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut saved = PersistedComposedView {
            version: state_file::STATE_VERSION,
            model_id: p.model_id.clone(),
            revision: p.revision,
            context,
            view,
            canonical_pins,
            anchor_pins: anchors,
            constraint_sets: vec![],
            dismissed_proposals: vec![proposal_identity(&p, &native)],
            accepted,
        };
        let mut constraints = ComposedConstraints {
            context: saved.context.clone(),
            pins: saved.canonical_pins.clone(),
            anchors: saved.anchor_pins.clone(),
            ownership: BTreeMap::new(),
            presentation: BTreeMap::new(),
        };
        verify_constraint_ownership(&input, &p, &mut constraints, &AtomicBool::new(false)).unwrap();
        saved.constraint_sets.push(constraints.persisted());
        saved.validate(&p, &AtomicBool::new(false)).unwrap();
        (input, saved)
    }
    fn write_saved(
        directory: &LocalTestDirectory,
        saved: &PersistedComposedView,
    ) -> std::path::PathBuf {
        let path = composed_state_path(&directory.0, &saved.model_id);
        state_file::save(&path, saved, &AtomicBool::new(false)).unwrap();
        path
    }
    fn accepted(app: &App) -> &NativeLayout {
        let viewer = app.world().resource::<Viewer>();
        &viewer.accepted.as_ref().expect(&viewer.message).1
    }
    fn canonical(app: &App) -> &layout::composed::ComposedSolution {
        accepted(app).composed_solution().unwrap()
    }
    fn restored_constraints(saved: &PersistedComposedView) -> ComposedConstraints {
        if let Some(set) = saved
            .constraint_sets
            .iter()
            .find(|set| set.context == saved.context)
        {
            return set.clone().into();
        }
        ComposedConstraints {
            context: saved.context.clone(),
            pins: saved.canonical_pins.clone(),
            anchors: saved.anchor_pins.clone(),
            ownership: BTreeMap::new(),
            presentation: BTreeMap::new(),
        }
    }

    #[test]
    fn collapsed_pins_survive_restart_empty_caches_and_reopening_exactly() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input.clone(), &directory.0);
        let pin = saved.anchor_pins[0].clone();
        complete(&mut app, ViewerAction::Collapse("datastore:orders".into()));
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        assert!(canonical(&app).assembly.frame("datastore:orders").is_err());
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        let stored = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert!(!stored.canonical_pins.contains_key("datastore:orders"));
        assert_eq!(stored.constraint_sets[0].pins, saved.canonical_pins);
        assert_eq!(stored.constraint_sets[0].anchors, saved.anchor_pins);
        drop(app);
        let mut app = cold(input, &directory.0);
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.cache.clear();
            viewer.cache_order.clear();
            viewer.canonical_cache = Arc::new(layout::composed::CanonicalPlacementCache::default());
        }
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        pin.validate(&canonical(&app).assembly).unwrap();
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("datastore:orders")
                .unwrap()
                .solution()
                .boxes["table:customers"]
                .bounds
                .center(),
            pin.local_point()
        );
        assert_eq!(app.world().resource::<LayoutStatus>().save_failures, 0);
    }

    #[test]
    fn inactive_pinned_context_survives_enter_restart_and_return() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input.clone(), &directory.0);
        complete(&mut app, ViewerAction::Collapse("datastore:orders".into()));
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        let stored = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(stored.context.root, "datastore:orders");
        assert_eq!(
            stored.canonical_pins["datastore:orders"]["table:customers"],
            saved.canonical_pins["datastore:orders"]["table:customers"]
        );
        assert_eq!(stored.anchor_pins[0].anchor(), "datastore:orders");
        stored.anchor_pins[0]
            .validate(&canonical(&app).assembly)
            .unwrap();
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
        assert!(
            !stored
                .constraint_sets
                .iter()
                .any(|set| set.context.root == "datastore:orders")
        );
        assert!(
            stored
                .constraint_sets
                .iter()
                .any(|set| set.context == saved.context && set.pins == saved.canonical_pins)
        );
        drop(app);
        let mut app = cold(input, &directory.0);
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        complete(&mut app, ViewerAction::Back);
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        for pin in &saved.anchor_pins {
            pin.validate(&canonical(&app).assembly).unwrap();
        }
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
    }

    #[test]
    fn inherited_pins_are_read_only_and_origin_unpin_and_undo_refresh_descendants() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        assert!(is_pinned(
            app.world().resource::<Viewer>(),
            "table:customers"
        ));
        let before = std::fs::read(&path).unwrap();
        let old = accepted(&app).clone();
        for action in [
            ViewerAction::Unpin("table:customers".into()),
            ViewerAction::Pin("table:customers".into()),
        ] {
            complete(&mut app, action);
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .as_ref()
                    .is_some_and(
                        |error| error.contains("scope:orders") && error.contains("Shift+P")
                    )
            );
            assert_eq!(accepted(&app), &old);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        complete(&mut app, ViewerAction::Back);
        complete(&mut app, ViewerAction::Unpin("table:customers".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        assert!(!is_pinned(
            app.world().resource::<Viewer>(),
            "table:customers"
        ));
        assert!(
            accepted(&app)
                .composed_constraints()
                .unwrap()
                .pins
                .is_empty()
        );
        complete(&mut app, ViewerAction::Undo); // Return to the unpinned parent.
        complete(&mut app, ViewerAction::Undo); // Restore its original pin.
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        assert!(is_pinned(
            app.world().resource::<Viewer>(),
            "table:customers"
        ));
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("datastore:orders")
                .unwrap()
                .solution()
                .boxes["table:customers"]
                .bounds
                .center(),
            saved.canonical_pins["datastore:orders"]["table:customers"]
        );
    }

    #[test]
    fn conflicting_inherited_and_local_pins_preserve_scene_and_file_until_origin_is_unpinned() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let mut child = saved.constraint_sets[0].clone();
        child.context.root = "datastore:orders".into();
        child.pins.retain(|owner, _| owner == "datastore:orders");
        child
            .pins
            .get_mut("datastore:orders")
            .unwrap()
            .get_mut("table:customers")
            .unwrap()
            .x += 8;
        child.anchors.clear();
        child.ownership.retain(|id, _| id == "table:customers");
        let path = write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        let before = std::fs::read(&path).unwrap();
        let old = accepted(&app).clone();
        // A conflicting request is never a valid v4 persisted state. Stage it
        // after accepting the parent to exercise transactional failure.
        app.world_mut()
            .resource_mut::<Viewer>()
            .composed_constraints
            .insert(
                (child.context.lens, child.context.root.clone()),
                child.into(),
            );
        let registry = app
            .world()
            .resource::<Viewer>()
            .accepted_composed_constraints
            .as_ref()
            .clone();
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .error
                .as_ref()
                .is_some_and(|error| error.contains("conflicting canonical coordinates"))
        );
        assert_eq!(accepted(&app), &old);
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints,
            registry
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        complete(&mut app, ViewerAction::Unpin("table:customers".into()));
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        assert!(!is_pinned(
            app.world().resource::<Viewer>(),
            "table:customers"
        ));
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
    }

    #[test]
    fn version_two_descendant_state_repairs_with_inherited_pins_before_upgrade() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input.clone(), &directory.0);
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        drop(app);
        let mut old: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        old["version"] = 2.into();
        old["canonical_pins"] = serde_json::json!({});
        old["anchor_pins"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let prior = std::fs::read(&path).unwrap();
        let loaded = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(loaded.version, 2);
        assert_eq!(std::fs::read(&path).unwrap(), prior);
        let app = cold(input, &directory.0);
        assert!(
            app.world().resource::<LayoutStatus>().error.is_none(),
            "{}",
            app.world().resource::<Viewer>().message
        );
        assert!(is_pinned(
            app.world().resource::<Viewer>(),
            "table:customers"
        ));
        let updated = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(updated.version, state_file::STATE_VERSION);
        assert_eq!(
            updated.canonical_pins["datastore:orders"],
            saved.canonical_pins["datastore:orders"]
        );
        assert_eq!(updated.constraint_sets.len(), 1);
    }

    fn assert_ready(app: &App) {
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
    }

    fn child_origin_pin(app: &mut App) -> ComposedConstraints {
        complete(app, ViewerAction::Enter("datastore:orders".into()));
        complete(app, ViewerAction::Expand("table:orders".into()));
        complete(app, ViewerAction::Pin("column:customer-id".into()));
        assert_ready(app);
        accepted(app).owned_constraints().unwrap()
    }

    #[test]
    fn child_origin_pin_survives_parent_return_collapse_cold_restore_and_origin_undo() {
        let file = LocalTestFile::new();
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let input = LayoutInput::bundle(file.0.clone());
        let directory = LocalTestDirectory::new();
        let mut app = cold(input.clone(), &directory.0);
        let root_before = canonical(&app)
            .assembly
            .frame("scope:orders")
            .unwrap()
            .solution()
            .boxes
            .clone();
        let child = child_origin_pin(&mut app);
        let pin = &child.anchors[0];
        complete(&mut app, ViewerAction::Back);
        assert_ready(&app);
        assert!(canonical(&app).assembly.frame("datastore:orders").is_err());
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert_ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
        assert_eq!(
            accepted(&app).composed_constraints().unwrap().anchors,
            child.anchors
        );
        assert!(accepted(&app).owned_constraints().unwrap().pins.is_empty());
        for (id, bounds) in &root_before {
            if !matches!(id.as_str(), "scope:orders" | "datastore:orders") {
                assert_eq!(
                    &canonical(&app)
                        .assembly
                        .frame("scope:orders")
                        .unwrap()
                        .solution()
                        .boxes[id],
                    bounds,
                    "unrelated root object moved: {id}"
                );
            }
        }
        complete(&mut app, ViewerAction::Collapse("datastore:orders".into()));
        assert_ready(&app);
        drop(app);
        let mut app = cold(input, &directory.0);
        assert_ready(&app);
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert_ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints.len(),
            1
        );
        complete(
            &mut app,
            ViewerAction::OpenPinOrigin("column:customer-id".into()),
        );
        assert_ready(&app);
        assert_eq!(canonical(&app).assembly.root(), "datastore:orders");
        complete(&mut app, ViewerAction::Unpin("column:customer-id".into()));
        assert_ready(&app);
        assert!(
            app.world()
                .resource::<Viewer>()
                .composed_constraints
                .is_empty()
        );
        complete(&mut app, ViewerAction::Undo);
        assert_ready(&app);
        complete(&mut app, ViewerAction::Back);
        assert_ready(&app);
        pin.validate(&canonical(&app).assembly).unwrap();
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints.len(),
            1
        );
    }

    #[test]
    fn open_pin_origin_ignores_noncontributing_same_id_group_override() {
        let file = LocalTestFile::new();
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let directory = LocalTestDirectory::new();
        let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        let child = child_origin_pin(&mut app);
        complete(&mut app, ViewerAction::Back);
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert_ready(&app);
        let mut unrelated = child;
        unrelated.context.root = "aaa-group:retained".into();
        let pins = unrelated.pins.remove("table:orders").unwrap();
        unrelated.pins.insert(unrelated.context.root.clone(), pins);
        unrelated.anchors.clear();
        app.world_mut()
            .resource_mut::<Viewer>()
            .composed_constraints
            .insert(
                (unrelated.context.lens, unrelated.context.root.clone()),
                unrelated,
            );
        assert_eq!(
            pin_origin(app.world().resource::<Viewer>(), "column:customer-id").as_deref(),
            Some("datastore:orders")
        );
        complete(
            &mut app,
            ViewerAction::OpenPinOrigin("column:customer-id".into()),
        );
        assert_ready(&app);
        assert_eq!(canonical(&app).assembly.root(), "datastore:orders");
        assert!(
            accepted(&app).owned_constraints().unwrap().pins["table:orders"]
                .contains_key("column:customer-id")
        );
    }

    #[test]
    fn canonical_root_pin_is_dormant_in_collapsed_parent_not_a_proxy_pin() {
        let file = LocalTestFile::new();
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let directory = LocalTestDirectory::new();
        let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        complete(&mut app, ViewerAction::Pin("datastore:orders".into()));
        assert_ready(&app);
        let owned = accepted(&app).owned_constraints().unwrap();
        let point = owned.pins["datastore:orders"]["datastore:orders"];
        complete(&mut app, ViewerAction::Back);
        assert_ready(&app);
        let LayoutData::Composed { pins, anchors, .. } = &accepted(&app).data else {
            panic!("composed")
        };
        assert!(pins.is_empty());
        assert!(anchors.is_empty());
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert_ready(&app);
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("datastore:orders")
                .unwrap()
                .bounds()
                .center(),
            point
        );
        assert!(
            !accepted(&app)
                .composed_constraints()
                .unwrap()
                .pins
                .contains_key("scope:orders")
        );
    }

    #[test]
    fn version_three_parent_repairs_with_child_origin_pin_before_upgrade() {
        let file = LocalTestFile::new();
        file.write(include_bytes!(
            "../../../public/examples/data-messaging.bundle.jsonl"
        ));
        let directory = LocalTestDirectory::new();
        let input = LayoutInput::bundle(file.0.clone());
        let mut app = cold(input.clone(), &directory.0);
        let child = child_origin_pin(&mut app);
        complete(&mut app, ViewerAction::Back);
        complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
        assert_ready(&app);
        let path = composed_state_path(&directory.0, &accepted(&app).model_id);
        drop(app);
        let mut wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        wire["version"] = 3.into();
        wire["canonical_pins"] = serde_json::json!({});
        wire["anchor_pins"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            state_file::load(&path, &AtomicBool::new(false))
                .unwrap()
                .version,
            3
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let app = cold(input, &directory.0);
        assert_ready(&app);
        child.anchors[0]
            .validate(&canonical(&app).assembly)
            .unwrap();
        let stored = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(stored.version, state_file::STATE_VERSION);
        assert_eq!(stored.canonical_pins, child.pins);
        assert_eq!(stored.anchor_pins, child.anchors);
        assert_eq!(stored.constraint_sets.len(), 1);
        assert_eq!(stored.constraint_sets[0].context.root, "datastore:orders");
    }

    fn write_two_databases(file: &LocalTestFile, revision: u64, move_customers: bool) {
        let mut records: Vec<serde_json::Value> =
            include_str!("../../../public/examples/data-messaging.bundle.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        records[0]["value"]["revision"] = revision.into();
        let mut store = records
            .iter()
            .find(|record| record["value"]["summary"]["id"] == "datastore:orders")
            .unwrap()
            .clone();
        store["value"]["summary"]["id"] = "datastore:archive".into();
        store["value"]["summary"]["label"] = "Archive".into();
        records.push(store);
        let mut contains = records
            .iter()
            .find(|record| record["value"]["id"] == "rel:app-db")
            .unwrap()
            .clone();
        contains["value"]["id"] = "rel:app-archive".into();
        contains["value"]["target"] = "datastore:archive".into();
        records.push(contains);
        if move_customers {
            records
                .iter_mut()
                .find(|record| record["value"]["id"] == "rel:owns-customers")
                .unwrap()["value"]["source"] = "datastore:archive".into();
        }
        let entities = records
            .iter()
            .filter(|record| record["record"] == "entity")
            .count();
        let relations = records
            .iter()
            .filter(|record| record["record"] == "relation")
            .count();
        records
            .iter_mut()
            .find(|record| record["record"] == "complete")
            .unwrap()["value"] =
            serde_json::json!({"entities":entities,"relations":relations,"evidence":0});
        records.sort_by_key(|record| match record["record"].as_str() {
            Some("manifest") => (0, String::new()),
            Some("entity") => (
                1,
                record["value"]["summary"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
            Some("relation") => (2, record["value"]["id"].as_str().unwrap().to_owned()),
            Some("complete") => (3, String::new()),
            _ => panic!("unexpected record kind in declared fixture"),
        });
        let mut bytes = Vec::new();
        for record in records {
            serde_json::to_writer(&mut bytes, &record).unwrap();
            bytes.push(b'\n');
        }
        file.replace(&bytes);
    }

    #[test]
    fn ancestor_pin_reparented_into_current_descendant_blocks_reload_and_cold_restore() {
        for restart in [false, true] {
            let file = LocalTestFile::new();
            write_two_databases(&file, 1, false);
            let directory = LocalTestDirectory::new();
            let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
            complete(&mut app, ViewerAction::Expand("datastore:orders".into()));
            complete(&mut app, ViewerAction::Pin("table:customers".into()));
            complete(&mut app, ViewerAction::Enter("datastore:archive".into()));
            assert!(
                app.world().resource::<LayoutStatus>().error.is_none(),
                "{}",
                app.world().resource::<Viewer>().message
            );
            let old = accepted(&app).clone();
            assert!(old.composed_constraints().unwrap().pins.is_empty());
            let registry = app
                .world()
                .resource::<Viewer>()
                .composed_constraints
                .clone();
            assert!(!registry[&(SemanticLens::Logical, "scope:orders".into())]
                .ownership["table:customers"].contains(&"datastore:archive".into()));
            let path = composed_state_path(&directory.0, &old.model_id);
            let before = std::fs::read(&path).unwrap();
            write_two_databases(&file, 2, true);
            if restart {
                drop(app);
                app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
                assert!(app.world().resource::<Viewer>().accepted.is_none());
                assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
            } else {
                complete(&mut app, ViewerAction::Reload);
                assert_eq!(accepted(&app), &old);
            }
            let viewer = app.world().resource::<Viewer>();
            assert!(
                viewer.message.contains("ownership changed"),
                "{}",
                viewer.message
            );
            assert_eq!(viewer.composed_constraints, registry);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn child_context_moved_into_current_ancestor_blocks_reload_and_restart() {
        for restart in [false, true] {
            let file = LocalTestFile::new();
            write_two_databases(&file, 1, false);
            let directory = LocalTestDirectory::new();
            let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
            complete(&mut app, ViewerAction::Enter("table:customers".into()));
            complete(&mut app, ViewerAction::Pin("table:customers".into()));
            complete(&mut app, ViewerAction::Enter("datastore:archive".into()));
            assert_ready(&app);
            let old = accepted(&app).clone();
            assert!(old.composed_constraints().unwrap().pins.is_empty());
            let registry = app
                .world()
                .resource::<Viewer>()
                .composed_constraints
                .clone();
            let path = composed_state_path(&directory.0, &old.model_id);
            let before = std::fs::read(&path).unwrap();
            write_two_databases(&file, 2, true);
            if restart {
                drop(app);
                app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
                assert!(app.world().resource::<Viewer>().accepted.is_none());
                assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
            } else {
                complete(&mut app, ViewerAction::Reload);
                assert_eq!(accepted(&app), &old);
            }
            let viewer = app.world().resource::<Viewer>();
            assert!(
                viewer.message.contains("ownership changed"),
                "{}",
                viewer.message
            );
            assert_eq!(viewer.composed_constraints, registry);
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn hidden_reparenting_is_a_conflict_and_preserves_the_previous_file_and_pins() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        complete(&mut app, ViewerAction::Collapse("datastore:orders".into()));
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
        drop(app);
        let before = std::fs::read(&path).unwrap();
        write_two_databases(&file, 2, true);
        let app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_none());
        assert!(
            viewer.message.contains("ownership changed"),
            "{}",
            viewer.message
        );
        assert_eq!(
            viewer.composed_constraints[&(saved.context.lens, saved.context.root.clone())],
            restored_constraints(&saved)
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
    }

    #[test]
    fn pin_registry_limit_rejects_before_publication_without_losing_existing_overrides() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let directory = LocalTestDirectory::new();
        let mut app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        let old = accepted(&app).clone();
        let path = composed_state_path(&directory.0, &old.model_id);
        let before = std::fs::read(&path).unwrap();
        let context = old.composed_constraints().unwrap().context;
        let registry: BTreeMap<_, _> = (0..64)
            .map(|i| {
                let mut context = context.clone();
                context.root = format!("unused-context:{i}");
                (
                    (context.lens, context.root.clone()),
                    ComposedConstraints {
                        context,
                        pins: BTreeMap::from([(
                            "root".into(),
                            BTreeMap::from([("a".into(), Point::ZERO)]),
                        )]),
                        anchors: vec![],
                        ownership: BTreeMap::new(),
                        presentation: BTreeMap::new(),
                    },
                )
            })
            .collect();
        app.world_mut()
            .resource_mut::<Viewer>()
            .composed_constraints = registry.clone();
        let attempts = app.world().resource::<LayoutStatus>().save_attempts;
        complete(&mut app, ViewerAction::Pin("a".into()));
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.job.is_none());
        assert_eq!(accepted(&app), &old);
        assert_eq!(viewer.composed_constraints, registry);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .as_ref()
                .is_some_and(|e| e.contains("64 view contexts"))
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().save_attempts,
            attempts
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut other = context;
        other.root = "another-context".into();
        let registry = BTreeMap::from([(
            (other.lens, other.root.clone()),
            ComposedConstraints {
                context: other,
                pins: BTreeMap::from([(
                    "root".into(),
                    (0..layout::assembly::MAX_TOTAL_BOXES)
                        .map(|i| (format!("pin:{i}"), Point::ZERO))
                        .collect(),
                )]),
                anchors: vec![],
                ownership: BTreeMap::new(),
                presentation: BTreeMap::new(),
            },
        )]);
        app.world_mut()
            .resource_mut::<Viewer>()
            .composed_constraints = registry.clone();
        complete(&mut app, ViewerAction::Pin("a".into()));
        assert_eq!(accepted(&app), &old);
        assert_eq!(
            app.world().resource::<Viewer>().composed_constraints,
            registry
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .as_ref()
                .is_some_and(|e| e.contains("too many"))
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().save_attempts,
            attempts
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn startup_restores_exact_canonical_geometry_pins_context_and_dismissals() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let app = cold(input, &directory.0);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert_eq!(
            status.layout_timings.back().unwrap().cache_outcome,
            "saved_exact"
        );
        assert_eq!(status.layout_timings.back().unwrap().solve_ms, 0.0);
        assert_eq!(canonical(&app), &saved.accepted);
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(
            viewer.canonical_cache.len(),
            saved.accepted.assembly.frame_ids().count()
        );
        assert_eq!(viewer.view.expanded, saved.view.expanded);
        assert_eq!(viewer.view.breadcrumbs(), saved.view.breadcrumbs());
        assert_eq!(
            viewer.dismissed.iter().cloned().collect::<Vec<_>>(),
            saved.dismissed_proposals
        );
        assert_eq!(
            viewer.composed_constraints[&(saved.context.lens, saved.context.root.clone())],
            restored_constraints(&saved)
        );
        assert_eq!(status.save_attempts, 1);
        assert_eq!(status.save_failures, 0);
        let mut on_disk = state_file::load(path, &AtomicBool::new(false)).unwrap();
        on_disk
            .validate(
                &viewer.accepted.as_ref().unwrap().0,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(on_disk.accepted, saved.accepted);
    }

    #[test]
    fn restored_interiors_are_warmed_and_apply_undo_publish_their_actual_geometry() {
        use layout::composed::{
            CanonicalPlacementCache, ComposedReuse, solve_composed_cached,
            solve_composed_reorganized,
        };
        let file = LocalTestFile::new();
        let input = saved_region_membership_fixture(&file, false);
        let mut view = ViewState::default();
        view.semantic_zoom = false;
        view.frame_lens = Some(SemanticLens::Logical);
        view.enter("root");
        view.set_expanded("a", true);
        view.set_expanded("b", true);
        let cancel = AtomicBool::new(false);
        let p = engine_projection(
            input.project(&view, &cancel).unwrap(),
            LayoutEngine::Composed,
        );
        let context = CompositionContext {
            model_id: p.model_id.clone(),
            root: p.root.clone(),
            lens: p.lens,
            policy: view.policy,
        };
        let pins = CanonicalPins::new();
        let temporary = BTreeMap::from([(
            "a".into(),
            BTreeMap::from([("a1".into(), Point::new(600, 0, 0))]),
        )]);
        let mut cache = CanonicalPlacementCache::default();
        let request = |pins| ComposedRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins,
            work_budget: 192,
            cancelled: &cancel,
        };
        let pinned = solve_composed_cached(
            request(&temporary),
            ComposedReuse {
                context: &context,
                previous: None,
                anchor_pins: &[],
            },
            &mut cache,
        )
        .unwrap();
        // The unpinned accepted arrangement remains deliberately off-center;
        // a fresh deterministic solve therefore cannot accidentally pass this test.
        let old = solve_composed_cached(
            request(&pins),
            ComposedReuse {
                context: &context,
                previous: Some((&context, &pinned)),
                anchor_pins: &[],
            },
            &mut cache,
        )
        .unwrap();
        let new = solve_composed_reorganized(
            request(&pins),
            ComposedReuse {
                context: &context,
                previous: Some((&context, &old)),
                anchor_pins: &[],
            },
            &cache,
        )
        .unwrap();
        let old_leaf = old.assembly.frame("a").unwrap().solution().boxes["a1"].clone();
        let new_leaf = new.assembly.frame("a").unwrap().solution().boxes["a1"].clone();
        assert_ne!(old_leaf, new_leaf);
        let directory = LocalTestDirectory::new();
        let saved = PersistedComposedView {
            version: state_file::STATE_VERSION,
            model_id: p.model_id.clone(),
            revision: p.revision,
            context: context.clone(),
            view,
            canonical_pins: pins.clone(),
            anchor_pins: vec![],
            constraint_sets: vec![],
            dismissed_proposals: vec![],
            accepted: old,
        };
        write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        assert_eq!(canonical(&app), &saved.accepted);
        complete(&mut app, ViewerAction::Enter("a".into()));
        assert_eq!(canonical(&app).work.cache.hits, 1);
        assert_eq!(canonical(&app).work.evaluations["a"], 0);
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("a")
                .unwrap()
                .solution()
                .boxes["a1"],
            old_leaf
        );
        complete(&mut app, ViewerAction::Back);
        let child_key = app
            .world()
            .resource::<Viewer>()
            .cache
            .iter()
            .find(|(_, entry)| entry.0.root == "a")
            .unwrap()
            .0
            .clone();
        // Feed a fully validated candidate to the actual Apply action. The
        // improvement gate is tested independently; this checks publication.
        app.world_mut().resource_mut::<Viewer>().proposal =
            Some(NativeLayout::composed(new.clone(), pins, vec![], context, &cancel).unwrap());
        complete(&mut app, ViewerAction::Apply);
        assert_eq!(canonical(&app), &new);
        assert!(
            !app.world()
                .resource::<Viewer>()
                .cache
                .contains_key(&child_key)
        );
        complete(&mut app, ViewerAction::Enter("a".into()));
        assert_eq!(canonical(&app).work.cache.hits, 1);
        assert_eq!(canonical(&app).work.evaluations["a"], 0);
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("a")
                .unwrap()
                .solution()
                .boxes["a1"],
            new_leaf
        );
        // Undo child navigation, then undo Apply through the real history.
        complete(&mut app, ViewerAction::Undo);
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(canonical(&app), &saved.accepted);
        assert!(
            !app.world()
                .resource::<Viewer>()
                .cache
                .contains_key(&child_key)
        );
        complete(&mut app, ViewerAction::Enter("a".into()));
        assert_eq!(canonical(&app).work.cache.hits, 1);
        assert_eq!(canonical(&app).work.evaluations["a"], 0);
        assert_eq!(
            canonical(&app)
                .assembly
                .frame("a")
                .unwrap()
                .solution()
                .boxes["a1"],
            old_leaf
        );
        assert!(app.world().resource::<LayoutStatus>().error.is_none());
    }

    #[test]
    fn saved_constraints_survive_navigation_unpin_undo_and_restart() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        let expected = accepted(&app).clone();
        // Return to independently pinned root geometry after inspecting a child.
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        complete(&mut app, ViewerAction::Back);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        assert_eq!(accepted(&app), &expected);
        complete(&mut app, ViewerAction::Unpin("service:orders".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        let current = accepted(&app).composed_constraints().unwrap();
        assert!(
            !current
                .pins
                .get(&saved.context.root)
                .is_some_and(|pins| pins.contains_key("service:orders"))
        );
        assert_eq!(current.anchors, saved.anchor_pins);
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        assert_eq!(accepted(&app), &expected);
        drop(app);
        let reopened = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        assert_eq!(accepted(&reopened), &expected);
    }

    #[test]
    fn newer_same_context_revision_repairs_saved_interiors_with_independent_pins() {
        let file = LocalTestFile::new();
        let (_, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let bytes = include_str!("../../../public/examples/data-messaging.bundle.jsonl");
        file.replace(
            bytes
                .replacen("\"revision\":1", "\"revision\":2", 1)
                .as_bytes(),
        );
        let app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert_eq!(
            status.layout_timings.back().unwrap().cache_outcome,
            "saved_repair"
        );
        assert_eq!(canonical(&app).revision, 2);
        for id in saved.accepted.assembly.frame_ids() {
            assert_eq!(
                canonical(&app).assembly.frame(id).unwrap().solution().boxes,
                saved.accepted.assembly.frame(id).unwrap().solution().boxes
            );
            assert_eq!(
                canonical(&app).assembly.instance(id).unwrap(),
                saved.accepted.assembly.instance(id).unwrap()
            );
        }
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        assert!(canonical(&app).work.cache.previous_hits > 0);
        assert_eq!(
            state_file::load(path, &AtomicBool::new(false))
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn pinned_return_uses_same_context_predecessor_when_the_whole_view_key_changes() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        let band = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0
            .nodes
            .iter()
            .find(|node| node.id == "service:orders")
            .unwrap()
            .band;
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        // An explicit preference equal to the current band changes the full-view
        // cache key, but neither the typed context nor legal canonical placement.
        app.world_mut()
            .resource_mut::<Viewer>()
            .view
            .layer_preferences
            .insert("service:orders".into(), band);
        complete(&mut app, ViewerAction::Back);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        assert_ne!(
            canonical(&app).projection_key,
            saved.accepted.projection_key
        );
        assert!(canonical(&app).work.cache.previous_hits > 0);
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        for id in saved.accepted.assembly.frame_ids() {
            assert_eq!(
                canonical(&app).assembly.frame(id).unwrap().solution().boxes,
                saved.accepted.assembly.frame(id).unwrap().solution().boxes
            );
        }
    }

    #[test]
    fn a_removed_pinned_object_preserves_saved_constraints_and_file_after_failed_repair() {
        use visualyze_model::provider::{BundleRecord, BundleWriter, Limits};
        let file = LocalTestFile::new();
        let (_, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let before = std::fs::read(&path).unwrap();
        let removed = ["table:customers", "column:customer-id"];
        let staged = LocalTestFile::new();
        let mut writer = None;
        for line in include_str!("../../../public/examples/data-messaging.bundle.jsonl").lines() {
            let record: BundleRecord = serde_json::from_str(line).unwrap();
            match record {
                BundleRecord::Manifest(mut manifest) => {
                    manifest.revision = 2;
                    writer =
                        Some(BundleWriter::create(&staged.0, manifest, Limits::default()).unwrap());
                }
                BundleRecord::Complete(_) => (),
                BundleRecord::Entity(ref entity)
                    if removed.contains(&entity.summary.id.as_str()) => {}
                BundleRecord::Relation(ref relation)
                    if removed.contains(&relation.source.as_str())
                        || removed.contains(&relation.target.as_str()) => {}
                record => writer.as_mut().unwrap().push(&record).unwrap(),
            }
        }
        writer.unwrap().finish().unwrap();
        std::fs::rename(&staged.0, &file.0).unwrap();
        let app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_none());
        assert!(
            viewer.message.contains("original state file preserved"),
            "{}",
            viewer.message
        );
        assert_eq!(
            viewer.composed_constraints[&(saved.context.lens, saved.context.root.clone())],
            restored_constraints(&saved)
        );
        assert_eq!(viewer.view.expanded, saved.view.expanded);
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
    }

    #[test]
    fn cancelled_new_pin_restores_the_accepted_registry_and_exact_scene() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut app = cold(input, &directory.0);
        let before = accepted(&app).clone();
        let bytes = std::fs::read(&path).unwrap();
        let input = app.world().resource::<LayoutInput>().clone();
        let signal = {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            assert!(
                apply_action(
                    ViewerAction::Pin("channel:orders".into()),
                    &input,
                    LayoutEngine::Composed,
                    &mut viewer
                )
                .unwrap()
            );
            assert!(is_pinned(&viewer, "channel:orders"));
            viewer.job.as_ref().unwrap().cancel.clone()
        };
        // Queue cancellation before polling, even if the worker has already
        // finished; no timing race may publish the staged extra constraint.
        complete(&mut app, ViewerAction::Cancel);
        assert!(signal.load(Ordering::Relaxed));
        assert_eq!(accepted(&app), &before);
        assert!(!is_pinned(
            app.world().resource::<Viewer>(),
            "channel:orders"
        ));
        assert_eq!(
            accepted(&app).composed_constraints().unwrap(),
            restored_constraints(&saved)
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn corrupt_exact_geometry_retains_saved_constraints_and_never_replaces_file() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let first = value["accepted"]["routes"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        let x = first["fragments"][0]["points"][0]["x"].as_i64().unwrap();
        first["fragments"][0]["points"][0]["x"] = (x + 1).into();
        let before = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &before).unwrap();
        let app = cold(input, &directory.0);
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_none());
        assert!(viewer.message.contains("original state file preserved"));
        assert_eq!(viewer.view.expanded, saved.view.expanded);
        assert_eq!(
            viewer.composed_constraints[&(saved.context.lens, saved.context.root.clone())],
            restored_constraints(&saved)
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
    }

    #[test]
    fn corrupt_unknown_and_newer_saved_files_fail_closed_without_default_scene() {
        let file = LocalTestFile::new();
        let (_, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        assert_eq!(
            path,
            saved_path(
                &directory.0,
                &format!("{}|Composed|private-state", saved.model_id)
            )
        );
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for mutation in 0..5 {
            let mut changed = value.clone();
            let bytes = match mutation {
                0 => b"do not overwrite this corrupt local state".to_vec(),
                1 => {
                    changed["version"] = 999.into();
                    serde_json::to_vec(&changed).unwrap()
                }
                2 => {
                    changed["revision"] = 2.into();
                    changed["accepted"]["revision"] = 2.into();
                    for f in changed["accepted"]["frames"].as_array_mut().unwrap() {
                        f["revision"] = 2.into();
                    }
                    serde_json::to_vec(&changed).unwrap()
                }
                3 => {
                    changed["model_id"] = "foreign-model".into();
                    changed["context"]["model_id"] = "foreign-model".into();
                    changed["accepted"]["model_id"] = "foreign-model".into();
                    for frame in changed["accepted"]["frames"].as_array_mut().unwrap() {
                        frame["model_id"] = "foreign-model".into();
                    }
                    for proposal in changed["dismissed_proposals"].as_array_mut().unwrap() {
                        proposal["model_id"] = "foreign-model".into();
                    }
                    serde_json::to_vec(&changed).unwrap()
                }
                _ => {
                    changed["accepted"]["quality_version"] = (layout::QUALITY_VERSION + 1).into();
                    serde_json::to_vec(&changed).unwrap()
                }
            };
            std::fs::write(&path, &bytes).unwrap();
            let app = cold(LayoutInput::bundle(file.0.clone()), &directory.0);
            assert!(app.world().resource::<Viewer>().accepted.is_none());
            assert!(app.world().resource::<LayoutStatus>().error.is_some());
            assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn composed_save_dispatch_has_independent_filename_and_preserves_flat_bytes() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        let directory = LocalTestDirectory::new();
        let flat = saved_pipeline(&input);
        let flat_path = saved_path(
            &directory.0,
            &state_identity(&flat.model_id, LayoutEngine::Floorplan),
        );
        layout::persistence::save(&flat_path, &flat).unwrap();
        let flat_before = std::fs::read(&flat_path).unwrap();
        let app = cold(input, &directory.0);
        assert!(accepted(&app).composed_solution().is_some());
        let path = composed_state_path(&directory.0, &flat.model_id);
        assert_ne!(path, flat_path);
        assert!(path.is_file());
        assert_eq!(std::fs::read(&flat_path).unwrap(), flat_before);
        assert_eq!(app.world().resource::<LayoutStatus>().save_failures, 0);
        let mut saved = state_file::load(&path, &AtomicBool::new(false)).unwrap();
        saved
            .validate(
                &app.world()
                    .resource::<Viewer>()
                    .accepted
                    .as_ref()
                    .unwrap()
                    .0,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(&saved.accepted, canonical(&app));
    }

    #[test]
    fn cancel_after_startup_metadata_recovery_preserves_constraints_without_publishing() {
        let file = LocalTestFile::new();
        let (input, saved) = nested_saved(&file);
        let directory = LocalTestDirectory::new();
        let path = write_saved(&directory, &saved);
        let before = std::fs::read(&path).unwrap();
        let (_send, receive) = mpsc::sync_channel(1);
        let signal = Arc::new(AtomicBool::new(false));
        let mut app = App::new();
        app.insert_resource(input)
            .insert_resource(LayoutEngine::Composed)
            .insert_resource(Viewer {
                storage: Some(directory.0.clone()),
                job: Some(Job {
                    action_id: None,
                    cancel: signal.clone(),
                    receive: Mutex::new(receive),
                    reorganize: false,
                    replacement_input: None,
                    restored_view: Arc::new(Mutex::new(Some(saved.view.clone()))),
                    restored_composed: Arc::new(Mutex::new(Some(vec![restored_constraints(
                        &saved,
                    )]))),
                    restored_dismissals: default(),
                    timing: default(),
                    canonical_cache: default(),
                }),
                ..default()
            })
            .init_resource::<LayoutStatus>()
            .init_resource::<ViewerActions>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, (process_actions, poll, navigation_status).chain());
        complete(&mut app, ViewerAction::Cancel);
        assert!(signal.load(Ordering::Relaxed));
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_none() && viewer.job.is_none());
        assert_eq!(viewer.view.expanded, saved.view.expanded);
        assert_eq!(viewer.view.focus, saved.view.focus);
        assert_eq!(
            viewer.composed_constraints[&(saved.context.lens, saved.context.root.clone())],
            restored_constraints(&saved)
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
    }
}
