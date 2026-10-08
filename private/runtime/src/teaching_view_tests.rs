mod teaching_view_tests {
    use super::*;
    use visualyze_model::walkthrough::{TeachingLayout, TeachingView, teaching_model};

    /// Two scopes under `system`: `s0` holds a and b, `s1` holds c and d. The flow
    /// a -> c -> d crosses from one scope to the other.
    fn model() -> visualyze_model::architecture::Architecture {
        let entity = |id: &str, kind: &str| {
            format!(r#"{{"id":"{id}","kind":"{kind}","label":"{id}","boundary":"internal","basis":"declared"}}"#)
        };
        let relation = |id: &str, kind: &str, source: &str, target: &str| {
            format!(r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"}}"#)
        };
        let entities = [
            r#"{"id":"system","kind":"scope","label":"System","boundary":"internal","subtype":"system","basis":"declared"}"#.to_owned(),
            entity("s0", "scope"),
            entity("s1", "scope"),
            entity("a", "service"),
            entity("b", "service"),
            entity("c", "service"),
            entity("d", "service"),
        ];
        let relations = [
            relation("own-s0", "contains", "system", "s0"),
            relation("own-s1", "contains", "system", "s1"),
            relation("own-a", "contains", "s0", "a"),
            relation("own-b", "contains", "s0", "b"),
            relation("own-c", "contains", "s1", "c"),
            relation("own-d", "contains", "s1", "d"),
            relation("a-c", "requests", "a", "c"),
            relation("c-d", "requests", "c", "d"),
            relation("b-a", "requests", "b", "a"),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"teach","revision":1,"system_id":"system","snapshot":{{"id":"teach-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        visualyze_model::architecture::Architecture::from_json(json.as_bytes())
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn stage(model: &visualyze_model::architecture::Architecture) -> TeachingStage {
        let view = TeachingView {
            id: "tv:flow".into(),
            title: "The flow".into(),
            members: vec!["a".into(), "c".into(), "d".into()],
            layout: TeachingLayout::Pipeline,
        };
        TeachingStage {
            input: LayoutInput::snapshot(teaching_model(model, &view).unwrap()),
            policy: layout::projection::LayoutPolicy::Pipeline,
        }
    }

    fn staged(teaching: Option<TeachingStage>) -> ViewerAction {
        ViewerAction::Stage(Box::new(StagedScene {
            teaching,
            ..default()
        }))
    }

    fn displayed(app: &App) -> BTreeSet<String> {
        let viewer = app.world().resource::<Viewer>();
        let (p, _) = viewer
            .accepted
            .as_ref()
            .unwrap_or_else(|| panic!("{}", viewer.message));
        p.nodes.iter().map(|node| node.id.clone()).collect()
    }

    fn saved_files(directory: &std::path::Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    fn showing(app: &App, input: &LayoutInput) -> bool {
        Arc::ptr_eq(&app.world().resource::<LayoutInput>().0, &input.0)
    }

    #[test]
    fn a_teaching_view_lays_out_flat_saves_nothing_and_returns_the_model() {
        let directory = LocalTestDirectory::new();
        let model = model();
        let original = LayoutInput::snapshot(self::model());
        let mut app = cold_app(original.clone(), &directory.0);
        let full = displayed(&app);
        assert!(full.contains("s0") && full.contains("s1"), "{full:?}");
        let saved = saved_files(&directory.0);
        assert!(!saved.is_empty(), "the published model's layout is saved");

        // Enter: the members stand alone, outside their scopes, in a pipeline.
        let teaching = stage(&model);
        complete(&mut app, staged(Some(teaching.clone())));
        let flat = displayed(&app);
        for id in ["a", "c", "d"] {
            assert!(flat.contains(id), "{id} missing from {flat:?}");
        }
        for id in ["s0", "s1", "b"] {
            assert!(!flat.contains(id), "{id} still shown in {flat:?}");
        }
        assert!(showing(&app, &teaching.input));
        // The flat layout is temporary: the saved layout is untouched while it shows.
        assert_eq!(saved_files(&directory.0), saved);
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(
            viewer.view.policy,
            layout::projection::LayoutPolicy::Pipeline
        );
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.model_id.as_deref(), Some("teach"));
        assert_eq!(status.revision, Some(1));

        // Another step in the same view keeps the same source.
        complete(&mut app, staged(Some(teaching.clone())));
        assert!(showing(&app, &teaching.input));
        assert_eq!(displayed(&app), flat);

        // Leave: the published model and its view come back.
        complete(&mut app, ViewerAction::LeaveTeaching);
        assert!(showing(&app, &original));
        assert_eq!(displayed(&app), full);
        assert!(app.world().resource::<Viewer>().teaching.is_none());
        // Leaving twice changes nothing.
        complete(&mut app, ViewerAction::LeaveTeaching);
        assert!(showing(&app, &original));

        // A step outside any view also returns to the model.
        complete(&mut app, staged(Some(teaching.clone())));
        assert!(showing(&app, &teaching.input));
        complete(&mut app, staged(None));
        assert!(showing(&app, &original));
        assert_eq!(displayed(&app), full);

        // Nothing the teaching view laid out reached the saved layout.
        let after = saved_files(&directory.0);
        assert_eq!(saved.keys().collect::<Vec<_>>(), after.keys().collect::<Vec<_>>());
        for (path, bytes) in &after {
            let state: layout::persistence::PersistedView =
                serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            for id in ["s0", "s1"] {
                assert!(state.accepted.boxes.contains_key(id), "{path:?} lost {id}");
            }
        }
    }

    #[test]
    fn reload_waits_until_the_teaching_view_is_left() {
        let directory = LocalTestDirectory::new();
        let model = model();
        let mut app = cold_app(LayoutInput::snapshot(self::model()), &directory.0);
        complete(&mut app, staged(Some(stage(&model))));
        complete(&mut app, ViewerAction::Reload);
        let status = app.world().resource::<LayoutStatus>();
        assert!(
            status
                .action_error
                .as_deref()
                .is_some_and(|e| e.contains("teaching view")),
            "{:?}",
            status.action_error
        );
    }
}
