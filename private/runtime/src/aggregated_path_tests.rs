mod aggregated_path_tests {
    use super::*;

    /// `sub` (collapsed by default) holds two services; `api` is a collapsed
    /// service exposing two interfaces. Three requests cross from `sub` to
    /// `api`, so the default view draws them as one aggregated line.
    pub(super) fn model() -> visualyze_model::architecture::Architecture {
        let entity = |id: &str, kind: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","label":"Label {id}","boundary":"internal","basis":"declared"{extra}}}"#
            )
        };
        let relation = |id: &str, kind: &str, source: &str, target: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"{extra}}}"#
            )
        };
        let entities = [
            entity("system", "scope", r#","subtype":"system""#),
            entity("sub", "scope", ""),
            entity("svc0", "service", ""),
            entity("svc1", "service", ""),
            entity("api", "service", ""),
            entity("if0", "interface", r#","subtype":"api""#),
            entity("if1", "interface", r#","subtype":"api""#),
            entity("web", "client", r#","subtype":"web""#),
        ];
        let relations = [
            relation("own-sub", "contains", "system", "sub", ""),
            relation("own-api", "contains", "system", "api", ""),
            relation("own-web", "contains", "system", "web", ""),
            relation("own-svc0", "contains", "sub", "svc0", ""),
            relation("own-svc1", "contains", "sub", "svc1", ""),
            relation("exp-if0", "exposes", "api", "if0", ""),
            relation("exp-if1", "exposes", "api", "if1", ""),
            relation("call-a", "requests", "svc0", "if0", r#","label":"Fetch""#),
            relation("call-b", "requests", "svc1", "if1", r#","label":"Store""#),
            relation("call-c", "requests", "svc0", "if1", r#","label":"Audit""#),
            relation("web-api", "requests", "web", "if0", ""),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"aggregated-path","revision":1,"system_id":"system","snapshot":{{"id":"agg-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        visualyze_model::architecture::Architecture::from_json(json.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn viewer(app: &App) -> &Viewer {
        app.world().resource::<Viewer>()
    }
    fn projection(app: &App) -> &DesktopProjection {
        &viewer(app)
            .accepted
            .as_ref()
            .unwrap_or_else(|| panic!("{}", viewer(app).message))
            .0
    }
    fn lines_carrying(app: &App, id: &str) -> Vec<Vec<String>> {
        projection(app)
            .edges
            .iter()
            .filter(|edge| edge.semantic_ids.iter().any(|member| member == id))
            .map(|edge| edge.semantic_ids.clone())
            .collect()
    }
    fn all_members(app: &App) -> Vec<String> {
        let p = projection(app);
        let mut ids: Vec<String> = p
            .edges
            .iter()
            .flat_map(|edge| edge.semantic_ids.iter().cloned())
            .chain(p.internal_relation_ids.iter().cloned())
            .collect();
        ids.sort();
        ids
    }
    fn expanded_nodes(app: &App) -> BTreeSet<String> {
        projection(app)
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect()
    }
    fn message(app: &App) -> &str {
        &viewer(app).message
    }

    /// These tests are about aggregated lines, which need `sub` closed, so the
    /// view collapses it explicitly; the projector would otherwise open it
    /// (see `planned_detail_tests`).
    pub(super) fn collapsed_sub() -> ViewState {
        let mut view = ViewState::default();
        view.set_expanded("sub", false);
        view
    }

    fn flat_app() -> App {
        action_app(LayoutInput::snapshot(model()), collapsed_sub())
    }

    fn composed_app(directory: &std::path::Path) -> App {
        let input = LayoutInput::snapshot(model());
        let mut view = collapsed_sub();
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
            .init_resource::<Assets<crate::style::GlassMaterial>>()
            .add_systems(Update, (process_actions, poll, navigation_status).chain());
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        while app.world().resource::<Viewer>().job.is_some() {
            app.update();
            assert!(Instant::now() < deadline, "composed startup timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.update();
        app
    }

    #[test]
    fn details_list_the_members_of_an_aggregated_path_with_original_directions() {
        let mut app = flat_app();
        let before = all_members(&app);
        assert_eq!(
            lines_carrying(&app, "call-a"),
            [["call-a", "call-b", "call-c"]]
        );
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        complete(&mut app, ViewerAction::SelectRelation("call-b".into()));
        app.update();
        let details = app.world().get::<Text>(inspector).unwrap().0.clone();
        assert!(
            details.contains("This line carries 3 relationships"),
            "{details}"
        );
        // Original endpoints, predicate and evidence are shown per member, and
        // the selected one is marked.
        assert!(details.contains("call-a: svc0 --requests--> if0 (Fetch)"), "{details}");
        assert!(details.contains("> call-b: svc1 --requests--> if1 (Store)"), "{details}");
        assert!(details.contains("call-c: svc0 --requests--> if1 (Audit)"), "{details}");
        assert!(details.contains("evidence []"), "{details}");
        assert_eq!(
            app.world().resource::<LayoutStatus>().inspected_relation_ids,
            ["call-a", "call-b", "call-c"]
        );
        // A component selected without a path keeps the previous listing.
        complete(&mut app, ViewerAction::ClearRelationSelection);
        app.update();
        let details = app.world().get::<Text>(inspector).unwrap().0.clone();
        assert!(!details.contains("This line carries"), "{details}");
        assert!(details.contains("linked claims (page 1, B / N to browse"));
        assert_eq!(all_members(&app), before);
    }

    #[test]
    fn long_aggregated_paths_page_through_all_members() {
        let mut app = flat_app();
        // Pretend the line carries more members than one page holds.
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            let (p, _) = viewer.accepted.as_mut().unwrap();
            let template = p.claims["call-a"].clone();
            for n in 0..4 {
                let mut claim = template.clone();
                claim.id = format!("call-extra-{n}");
                p.claims.insert(claim.id.clone(), claim);
                p.graph.edges[0].semantic_ids.push(format!("call-extra-{n}"));
            }
        }
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        complete(&mut app, ViewerAction::SelectRelation("call-a".into()));
        app.update();
        let first = app.world().get::<Text>(inspector).unwrap().0.clone();
        assert!(first.contains("This line carries 7 relationships"), "{first}");
        assert!(first.contains("page 1 of 3"), "{first}");
        assert!(!first.contains("call-extra-3"), "{first}");
        complete(&mut app, ViewerAction::NextClaims);
        complete(&mut app, ViewerAction::NextClaims);
        complete(&mut app, ViewerAction::NextClaims);
        app.update();
        let last = app.world().get::<Text>(inspector).unwrap().0.clone();
        assert!(last.contains("page 3 of 3"), "{last}");
        assert!(last.contains("call-extra-3"), "{last}");
    }

    fn expand_both_ends(app: &mut App) {
        let before = all_members(app);
        let nodes_before = expanded_nodes(app);
        assert!(!nodes_before.contains("svc0") && !nodes_before.contains("if0"));
        complete(app, ViewerAction::SelectRelation("call-a".into()));
        complete(app, ViewerAction::ExpandPathEndpoints);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        // Both the scope and the service are open, so the members are three
        // separate lines between their original endpoints.
        let nodes = expanded_nodes(app);
        for id in ["svc0", "svc1", "if0", "if1"] {
            assert!(nodes.contains(id), "{id} missing from {nodes:?}");
        }
        for (id, source, target) in [
            ("call-a", "svc0", "if0"),
            ("call-b", "svc1", "if1"),
            ("call-c", "svc0", "if1"),
        ] {
            let lines = lines_carrying(app, id);
            assert_eq!(lines, [[id]], "{id} must be its own line");
            let edge = projection(app)
                .edges
                .iter()
                .find(|edge| edge.id == id)
                .unwrap();
            assert_eq!((edge.source.as_str(), edge.target.as_str()), (source, target));
        }
        // The visible count changed; membership did not.
        assert_eq!(all_members(app), before);
        assert!(
            message(app).contains("Expanded both ends of the selected path")
                && message(app).contains("its 3 relationships are now shown as 3 lines"),
            "{}",
            message(app)
        );
    }

    #[test]
    fn expand_both_endpoints_splits_an_aggregated_path_in_the_flat_engine() {
        let mut app = flat_app();
        expand_both_ends(&mut app);
    }

    #[test]
    fn expand_both_endpoints_splits_an_aggregated_path_in_the_composed_engine() {
        let directory = LocalTestDirectory::new();
        let mut app = composed_app(&directory.0);
        expand_both_ends(&mut app);
    }

    #[test]
    fn leaf_and_reference_endpoints_are_skipped_with_an_explanation() {
        let mut app = flat_app();
        // `web` is a leaf box; only the service end can open.
        complete(&mut app, ViewerAction::SelectRelation("web-api".into()));
        complete(&mut app, ViewerAction::ExpandPathEndpoints);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        assert!(expanded_nodes(&app).contains("if0"));
        assert!(!viewer(&app).view.is_expanded("web"));
        assert!(
            message(&app).starts_with("Expanded api;")
                && message(&app).contains("not expanded: web has no collapsed supplied children"),
            "{}",
            message(&app)
        );
        // With nothing left to open, the action reports why instead of acting.
        complete(&mut app, ViewerAction::SelectRelation("web-api".into()));
        complete(&mut app, ViewerAction::ExpandPathEndpoints);
        let status = app.world().resource::<LayoutStatus>();
        assert!(
            status
                .action_error
                .as_deref()
                .is_some_and(|error| error.contains("Neither end of the selected path can be expanded")),
            "{:?}",
            status.action_error
        );
        complete(&mut app, ViewerAction::ClearRelationSelection);
        complete(&mut app, ViewerAction::ExpandPathEndpoints);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .as_deref()
                .is_some_and(|error| error.contains("Select a path first"))
        );
    }

    #[test]
    fn reference_nodes_are_never_expanded() {
        let mut app = flat_app();
        complete(&mut app, ViewerAction::Enter("sub".into()));
        // Inside `sub`, the calls leave through references to `api` interfaces.
        let reference = projection(&app)
            .references
            .keys()
            .next()
            .cloned()
            .expect("entering sub leaves references to outside objects");
        let p = projection(&app);
        assert_eq!(
            endpoint_expansion_blocker(p, &reference),
            Some("is a reference to an object outside this view; Enter it to see its interior")
        );
    }
}
