mod planned_detail_tests {
    use super::aggregated_path_tests::{collapsed_sub, model};
    use super::*;
    use layout::composed::persistence::{self as composed_file};

    /// Scopes `s0`, `s1`, ... under `system`, each holding the given number of
    /// services, all in one layer.
    fn scopes_model(sizes: &[usize]) -> visualyze_model::architecture::Architecture {
        let mut entities = vec![
            r#"{"id":"system","kind":"scope","label":"System","boundary":"internal","subtype":"system","basis":"declared"}"#.to_owned(),
        ];
        let mut relations = vec![];
        for (scope, size) in sizes.iter().enumerate() {
            entities.push(format!(
                r#"{{"id":"s{scope}","kind":"scope","label":"Scope {scope}","boundary":"internal","basis":"declared"}}"#
            ));
            relations.push(format!(
                r#"{{"id":"own-s{scope}","kind":"contains","source":"system","target":"s{scope}","basis":"declared"}}"#
            ));
            for i in 0..*size {
                entities.push(format!(
                    r#"{{"id":"s{scope}-svc{i:02}","kind":"service","label":"Service {scope}.{i}","boundary":"internal","basis":"declared"}}"#
                ));
                relations.push(format!(
                    r#"{{"id":"own-s{scope}-svc{i:02}","kind":"contains","source":"s{scope}","target":"s{scope}-svc{i:02}","basis":"declared"}}"#
                ));
            }
        }
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"scopes","revision":1,"system_id":"system","snapshot":{{"id":"scopes-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        visualyze_model::architecture::Architecture::from_json(json.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// `s0` holds eighteen services in one layer: noise 25, over the threshold
    /// of 10 but far under the fallback-grouping level, so shown whole when asked.
    fn wide_model() -> visualyze_model::architecture::Architecture {
        scopes_model(&[18])
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
    fn displayed(app: &App) -> BTreeSet<String> {
        projection(app)
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect()
    }
    fn inspector_text(app: &mut App, inspector: Entity) -> String {
        app.update();
        app.world().get::<Text>(inspector).unwrap().0.clone()
    }

    #[test]
    fn default_views_open_supplied_scopes_and_collapse_is_explicit() {
        let mut app = action_app(LayoutInput::snapshot(model()), ViewState::default());
        assert_eq!(projection(&app).auto_opened, BTreeSet::from(["sub".into()]));
        assert!(displayed(&app).contains("svc0") && displayed(&app).contains("svc1"));
        // The user's own state records nothing about the planner's choice.
        assert!(viewer(&app).view.expanded.is_empty());
        assert!(viewer(&app).view.collapsed.is_empty());
        assert!(viewer(&app).view.planned_ids().is_empty());
        // Details say so for the selected component, and E means collapse.
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        complete(&mut app, ViewerAction::Select("sub".into()));
        let text = inspector_text(&mut app, inspector);
        assert!(
            text.contains("Opened automatically to show supplied structure within the readability budget; E collapses it."),
            "{text}"
        );
        assert!(matches!(
            expansion_toggle(viewer(&app), "sub"),
            ViewerAction::Collapse(id) if id == "sub"
        ));
        complete(&mut app, ViewerAction::Collapse("sub".into()));
        assert!(viewer(&app).view.collapsed.contains("sub"));
        assert!(projection(&app).auto_opened.is_empty());
        assert!(!displayed(&app).contains("svc0"));
        let text = inspector_text(&mut app, inspector);
        assert!(!text.contains("Opened automatically"), "{text}");
        // An explicit expansion is the user's, not the planner's.
        assert!(matches!(
            expansion_toggle(viewer(&app), "sub"),
            ViewerAction::Expand(id) if id == "sub"
        ));
        complete(&mut app, ViewerAction::Expand("sub".into()));
        assert!(viewer(&app).view.expanded.contains("sub"));
        assert!(!viewer(&app).view.collapsed.contains("sub"));
        assert!(projection(&app).auto_opened.is_empty());
        assert!(displayed(&app).contains("svc0"));
        let text = inspector_text(&mut app, inspector);
        assert!(!text.contains("Opened automatically"), "{text}");
    }

    #[test]
    fn status_line_shows_the_noise_level_and_marks_dense_views() {
        assert_eq!(noise_label(0.0), "noise 0.0");
        assert_eq!(noise_label(4.24), "noise 4.2");
        assert_eq!(noise_label(10.0), "noise 10.0");
        assert_eq!(noise_label(31.4), "noise 31 (dense)");
        let app = action_app(LayoutInput::snapshot(model()), ViewState::default());
        let status = app.world().resource::<LayoutStatus>();
        let expected = layout::noise::visual_noise(&projection(&app).graph).total;
        assert_eq!(status.noise, expected);
        let line = brief_status(viewer(&app), status);
        assert!(
            line.ends_with(&format!("| revision 1 | {}", noise_label(expected))),
            "{line}"
        );
    }

    #[test]
    fn explicit_expansion_is_never_refused_on_noise() {
        let mut app = action_app(LayoutInput::snapshot(wide_model()), ViewState::default());
        // Eighteen services in one row would be noisy, so none are opened.
        assert!(projection(&app).auto_opened.is_empty());
        assert!(!displayed(&app).contains("s0-svc00"));
        assert!(app.world().resource::<LayoutStatus>().noise <= layout::noise::NOISE_THRESHOLD);
        assert!(app.world().resource::<LayoutStatus>().densest_interior.is_none());
        complete(&mut app, ViewerAction::Expand("s0".into()));
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        assert!(displayed(&app).contains("s0-svc00") && displayed(&app).contains("s0-svc17"));
        // The focused view stays quiet: the box counts once. Its interior is the
        // dense one, and the status line says so.
        assert!(status.noise <= layout::noise::NOISE_THRESHOLD, "{}", status.noise);
        let (id, interior) = status.densest_interior.clone().unwrap();
        assert_eq!(id, "s0");
        assert!(
            interior > layout::noise::NOISE_THRESHOLD,
            "{interior}"
        );
        let line = brief_status(viewer(&app), status);
        assert!(
            line.ends_with("| noise 0.0 | densest interior 25 (dense)"),
            "{line}"
        );
        // Details for the expanded box carry its interior noise line.
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        complete(&mut app, ViewerAction::Select("s0".into()));
        let text = inspector_text(&mut app, inspector);
        assert!(
            text.contains(
                "Interior: visual noise 25.0 (18 boxes, widest layer 18, 0 connectors) (dense)"
            ),
            "{text}"
        );
        // Collapsing again is just as free, and the note goes away.
        complete(&mut app, ViewerAction::Collapse("s0".into()));
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.noise <= layout::noise::NOISE_THRESHOLD);
        assert!(status.densest_interior.is_none());
        assert!(!brief_status(viewer(&app), status).contains("densest interior"));
        let text = inspector_text(&mut app, inspector);
        assert!(!text.contains("Interior:"), "{text}");
    }

    #[test]
    fn status_line_names_the_densest_interior_only_when_it_is_dense() {
        let mut app = action_app(LayoutInput::snapshot(wide_model()), ViewState::default());
        let mut status = LayoutStatus {
            noise: 4.2,
            densest_interior: Some(("s0".into(), 31.4)),
            ..LayoutStatus::default()
        };
        let line = brief_status(viewer(&app), &status);
        assert!(line.ends_with("| noise 4.2 | densest interior 31 (dense)"), "{line}");
        status.densest_interior = Some(("s0".into(), 6.0));
        assert!(!brief_status(viewer(&app), &status).contains("densest"));
        // An interior at the threshold is quiet, like a view at the threshold.
        status.densest_interior = Some(("s0".into(), layout::noise::NOISE_THRESHOLD));
        assert!(!brief_status(viewer(&app), &status).contains("densest"));
        complete(&mut app, ViewerAction::Expand("s0".into()));
    }

    #[test]
    fn expanding_a_sibling_explicitly_never_closes_a_planned_box() {
        // Two quiet scopes beside one whose eighteen services are dense.
        let mut app = action_app(
            LayoutInput::snapshot(scopes_model(&[3, 3, 18])),
            ViewState::default(),
        );
        let planned = BTreeSet::from(["s0".to_owned(), "s1".to_owned()]);
        assert_eq!(projection(&app).auto_opened, planned);
        for action in [
            ViewerAction::Expand("s2".into()),
            ViewerAction::Collapse("s2".into()),
            ViewerAction::Expand("s2".into()),
        ] {
            let expanding = matches!(action, ViewerAction::Expand(_));
            complete(&mut app, action);
            let status = app.world().resource::<LayoutStatus>();
            assert!(status.action_error.is_none(), "{:?}", status.action_error);
            assert_eq!(projection(&app).auto_opened, planned, "expanding={expanding}");
            for svc in ["s0-svc00", "s0-svc02", "s1-svc00", "s1-svc02"] {
                assert!(displayed(&app).contains(svc), "{svc}");
            }
            assert_eq!(displayed(&app).contains("s2-svc00"), expanding);
            assert!(status.noise <= layout::noise::NOISE_THRESHOLD);
            // The planned interiors are quiet; only s2 can be dense.
            let (id, noise) = status.densest_interior.clone().unwrap();
            assert_eq!(noise > layout::noise::NOISE_THRESHOLD, expanding);
            assert_eq!(id == "s2", expanding);
        }
        // Expanding a planned box makes it the user's; the other stays planned.
        complete(&mut app, ViewerAction::Expand("s0".into()));
        assert_eq!(
            projection(&app).auto_opened,
            BTreeSet::from(["s1".to_owned()])
        );
        assert!(displayed(&app).contains("s1-svc00"));
        assert!(viewer(&app).view.expanded.contains("s0"));
    }

    #[test]
    fn bundle_interiors_plan_for_themselves_and_memoized_views_keep_the_plan() {
        let input = LayoutInput::bundle(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../public/examples/data-messaging.bundle.jsonl"),
        );
        let cancel = AtomicBool::new(false);
        let mut view = ViewState::default();
        let root = input.project(&view, &cancel).unwrap();
        // One level opens at the root: the table the datastore reveals waits.
        assert_eq!(
            root.auto_opened,
            BTreeSet::from(["datastore:orders".into()])
        );
        // The memoized view returns the same plan.
        let mut timings = SourceTimings::default();
        let again = input.project_timed(&view, &cancel, &mut timings).unwrap();
        assert!(timings.projection_cache_hit);
        assert_eq!(again.auto_opened, root.auto_opened);
        // Entering the datastore plans its interior with its own budget.
        view.enter("datastore:orders");
        let interior = input.project(&view, &cancel).unwrap();
        assert_eq!(interior.root, "datastore:orders");
        assert_eq!(
            interior.auto_opened,
            BTreeSet::from(["table:orders".into()])
        );
        // Explicit collapse stays closed in every interior.
        view.set_expanded("table:orders", false);
        assert!(
            input
                .project(&view, &cancel)
                .unwrap()
                .auto_opened
                .is_empty()
        );
    }

    // ---- persistence migration (ADR 0007 "Consequences") -------------------

    /// `sub` and `grp` are scopes of two services each, plus a client and a
    /// service that call into them. A build before ADR 0007 shows both scopes
    /// closed until the user (or zoom) opens one.
    fn legacy_model() -> visualyze_model::architecture::Architecture {
        let entity = |id: &str, kind: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","label":"Label {id}","boundary":"internal","basis":"declared"{extra}}}"#
            )
        };
        let own = |source: &str, target: &str| {
            format!(
                r#"{{"id":"own-{target}","kind":"contains","source":"{source}","target":"{target}","basis":"declared"}}"#
            )
        };
        let call = |id: &str, source: &str, target: &str| {
            format!(
                r#"{{"id":"{id}","kind":"requests","source":"{source}","target":"{target}","basis":"declared"}}"#
            )
        };
        let entities = [
            entity("system", "scope", r#","subtype":"system""#),
            entity("sub", "scope", ""),
            entity("grp", "scope", ""),
            entity("svc0", "service", ""),
            entity("svc1", "service", ""),
            entity("g0", "service", ""),
            entity("g1", "service", ""),
            entity("api", "service", ""),
            entity("web", "client", r#","subtype":"web""#),
        ];
        let relations = [
            own("system", "sub"),
            own("system", "grp"),
            own("system", "api"),
            own("system", "web"),
            own("sub", "svc0"),
            own("sub", "svc1"),
            own("grp", "g0"),
            own("grp", "g1"),
            call("call-a", "web", "api"),
            call("call-b", "api", "svc0"),
            call("call-c", "svc1", "g0"),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"legacy-detail","revision":1,"system_id":"system","snapshot":{{"id":"legacy-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        visualyze_model::architecture::Architecture::from_json(json.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// What a build before ADR 0007 saved for a view: no planner, so `sub` was
    /// closed by default (closed explicitly while generating, then cleared from
    /// the saved view), a zoom `auto_expanded` entry that opened `grp`, and the
    /// peer ceiling and group target it used to write.
    fn legacy_view() -> ViewState {
        let mut view = collapsed_sub();
        view.semantic_zoom = false;
        assert!(view.update_zoom("grp", 200.0));
        view.peer_ceiling = 64;
        view.group_target = 32;
        view
    }

    fn assert_legacy_fields(json: &serde_json::Value) {
        assert_eq!(json["peer_ceiling"], 64);
        assert_eq!(json["group_target"], 32);
        assert_eq!(json["auto_expanded"], serde_json::json!(["grp"]));
        assert!(json.get("planned").is_none(), "{json}");
    }

    #[test]
    fn flat_state_saved_before_adr_0007_loads_keeps_its_pin_and_is_recomputed() {
        let input = LayoutInput::snapshot(legacy_model());
        // Generate what the old build accepted: `sub` closed.
        let app = action_app(input.clone(), legacy_view());
        let old = viewer(&app);
        let mut view = old.accepted_view.clone();
        let mut accepted = old.accepted.as_ref().unwrap().1.flat().unwrap().clone();
        assert!(!accepted.boxes.contains_key("svc0"));
        let offset = Point::new(400, 160, -240);
        for item in accepted.boxes.values_mut() {
            item.bounds = item.bounds.translated(offset);
        }
        for route in &mut accepted.routes {
            for point in &mut route.points {
                *point = *point + offset;
            }
            for port in &mut route.scope_ports {
                port.point = port.point + offset;
            }
        }
        view.pins
            .insert("web".into(), accepted.boxes["web"].bounds.center());
        view.save_frame_pins();
        let old_projection = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Floorplan,
        );
        accepted.projection_key = old_projection.key.clone();
        layout::validate(&old_projection, &accepted, &view.pins, 2).unwrap();
        // The saved view is the old default: nothing was ever collapsed.
        view.collapsed.clear();
        let saved = layout::persistence::PersistedView {
            version: layout::persistence::STATE_VERSION,
            model_id: accepted.model_id.clone(),
            revision: accepted.revision,
            view,
            accepted,
            dismissed_proposals: vec![],
        };
        assert_legacy_fields(&serde_json::to_value(&saved.view).unwrap());
        let directory = LocalTestDirectory::new();
        let path = saved_path(
            &directory.0,
            &state_identity(&saved.model_id, LayoutEngine::Floorplan),
        );
        layout::persistence::save(&path, &saved).unwrap();
        // The file on disk is the old shape, and the current loader takes it.
        let loaded = layout::persistence::load(&path).unwrap();
        assert_eq!(loaded.view.peer_ceiling, 64);
        assert_eq!(loaded.view.group_target, 32);

        let app = cold_app(input.clone(), &directory.0);
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        let viewer = viewer(&app);
        let timing = status.layout_timings.back().unwrap();
        // The projection key changed (`sub` now opens), so the accepted layout
        // is repaired from its positions instead of being reused verbatim.
        assert_eq!(timing.cache_outcome, "saved_repair");
        let (restored, repaired) = viewer.accepted.as_ref().expect(&viewer.message);
        assert_ne!(restored.key, saved.accepted.projection_key);
        assert_eq!(restored.auto_opened, BTreeSet::from(["sub".into()]));
        let repaired = repaired.flat().unwrap();
        assert!(repaired.boxes.contains_key("svc0") && repaired.boxes.contains_key("svc1"));
        assert_ne!(repaired.projection_key, saved.accepted.projection_key);
        // The pin survives and is still valid, and the zoom entry still applies.
        assert_eq!(viewer.view.pins, saved.view.pins);
        assert_eq!(viewer.view.frame_pins, saved.view.frame_pins);
        assert_eq!(
            repaired.boxes["web"].bounds.center(),
            saved.view.pins["web"]
        );
        layout::validate(restored, repaired, &viewer.view.pins, 2).unwrap();
        assert!(viewer.view.is_expanded("grp"));
        // The planner's choice is neither the user's nor saved.
        assert!(viewer.view.expanded.is_empty() && viewer.view.collapsed.is_empty());

        // The newly saved file keeps the legacy fields for older builds, never
        // holds the planned set, round-trips, and restores exactly next time.
        let current = layout::persistence::load(&path).unwrap();
        assert_ne!(
            current.accepted.projection_key,
            saved.accepted.projection_key
        );
        assert_eq!(current.view.pins, saved.view.pins);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"planned\""), "{text}");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_legacy_fields(&json["view"]);
        let again = directory.0.join("round-trip.json");
        layout::persistence::save(&again, &current).unwrap();
        let round: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&again).unwrap()).unwrap();
        assert_eq!(round["view"], json["view"]);
        assert_eq!(round["accepted"], json["accepted"]);
        let expected_key = current.accepted.projection_key.clone();
        drop(app);
        let app = cold_app(input, &directory.0);
        assert_eq!(
            app.world()
                .resource::<LayoutStatus>()
                .layout_timings
                .back()
                .unwrap()
                .cache_outcome,
            "saved_exact"
        );
        assert_eq!(
            projection(&app).key,
            expected_key,
            "the planner is deterministic across runs"
        );
    }

    fn composed_start(input: &LayoutInput, directory: &std::path::Path, view: ViewState) -> App {
        let mut viewer = Viewer {
            view,
            storage: Some(directory.into()),
            ..default()
        };
        start(input, LayoutEngine::Composed, &mut viewer, false);
        let mut app = App::new();
        app.insert_resource(input.clone())
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
    fn composed_state_v6_saved_before_adr_0007_loads_keeps_its_pin_and_is_recomputed() {
        let input = LayoutInput::snapshot(legacy_model());
        let directory = LocalTestDirectory::new();
        let path = composed_state_path(&directory.0, "legacy-detail");
        // Generate what the old build saved: `sub` closed, `api` zoom-expanded,
        // and a canonical pin.
        let mut app = composed_start(&input, &directory.0, legacy_view());
        complete(&mut app, ViewerAction::Pin("web".into()));
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.action_error.is_none(), "{:?}", status.action_error);
        let pinned_before = viewer(&app)
            .accepted
            .as_ref()
            .unwrap()
            .1
            .owned_constraints()
            .unwrap()
            .pins;
        assert!(pinned_before.values().any(|pins| pins.contains_key("web")));
        let old_key = projection(&app).key.clone();
        drop(app);
        // Rewrite the saved view into the old default (nothing was collapsed).
        let mut wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(wire["version"], 6);
        assert_eq!(composed_file::STATE_VERSION, 6);
        wire["view"]["collapsed"] = serde_json::json!([]);
        assert_legacy_fields(&wire["view"]);
        std::fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
        let legacy = composed_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(legacy.version, 6);
        assert_eq!(legacy.view.peer_ceiling, 64);
        assert_eq!(legacy.view.group_target, 32);
        assert_eq!(legacy.accepted.projection_key, old_key);
        assert!(
            !legacy
                .accepted
                .assembly
                .frame("system")
                .unwrap()
                .solution()
                .boxes
                .contains_key("svc0")
        );

        let app = composed_start(&input, &directory.0, ViewState::default());
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert_eq!(
            status.layout_timings.back().unwrap().cache_outcome,
            "saved_repair"
        );
        let viewer = viewer(&app);
        let (projection, accepted) = viewer.accepted.as_ref().expect(&viewer.message);
        // The key differs: `sub` opens now, so the old arrangement is repaired,
        // not reused verbatim.
        assert_ne!(projection.key, old_key);
        assert_eq!(projection.auto_opened, BTreeSet::from(["sub".into()]));
        assert!(accepted.geometry.boxes.contains_key("svc0"));
        // The pin is preserved and the zoom entry still applies.
        assert_eq!(accepted.owned_constraints().unwrap().pins, pinned_before);
        assert!(viewer.view.is_expanded("grp"));
        assert!(viewer.view.expanded.is_empty() && viewer.view.collapsed.is_empty());
        // The new save carries no planner state, keeps the legacy fields, and
        // restores exactly afterwards.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"planned\""), "{text}");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_legacy_fields(&json["view"]);
        let saved = composed_file::load(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(saved.version, composed_file::STATE_VERSION);
        assert_ne!(saved.accepted.projection_key, old_key);
        assert_eq!(saved.view.pins, legacy.view.pins);
        drop(app);
        let app = composed_start(&input, &directory.0, ViewState::default());
        assert_eq!(
            app.world()
                .resource::<LayoutStatus>()
                .layout_timings
                .back()
                .unwrap()
                .cache_outcome,
            "saved_exact"
        );
    }
}
