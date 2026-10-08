mod teaching_view_tests {
    use super::*;
    use crate::transition::{Choreography, Held, SceneTransition, Track};
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

    /// The cold app, plus the transition player on a clock the test moves.
    fn animated_app(directory: &std::path::Path, input: LayoutInput) -> App {
        let mut app = cold_app(input, directory);
        app.init_resource::<SceneTransition>()
            .init_resource::<Time>()
            .add_systems(PostUpdate, crate::transition::play);
        app.update();
        app
    }

    fn wait(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(seconds));
        app.update();
    }

    /// Display ID -> (translation now, track if one is playing).
    fn objects(app: &mut App) -> BTreeMap<String, (Vec3, Option<Track>)> {
        let mut query = app
            .world_mut()
            .query::<(&PlacedObject, &Transform, Option<&Track>)>();
        query
            .iter(app.world())
            .map(|(object, transform, track)| {
                (object.id.clone(), (transform.translation, track.cloned()))
            })
            .collect()
    }

    /// Display ID -> half extents as drawn now: the box's own size times its scale.
    fn drawn(app: &mut App) -> BTreeMap<String, Vec3> {
        let mut query = app.world_mut().query::<(&PlacedObject, &Transform)>();
        query
            .iter(app.world())
            .map(|(object, transform)| (object.id.clone(), object.half * transform.scale))
            .collect()
    }

    /// Display ID -> the half extents each box is built at in its scene.
    fn own_size(app: &mut App) -> BTreeMap<String, Vec3> {
        let mut query = app.world_mut().query::<&PlacedObject>();
        query
            .iter(app.world())
            .map(|object| (object.id.clone(), object.half))
            .collect()
    }

    fn leaving(app: &mut App) -> usize {
        let mut query = app
            .world_mut()
            .query_filtered::<&Track, (Without<PlacedObject>, Without<PlacedRelation>)>();
        query.iter(app.world()).filter(|t| t.despawn).count()
    }

    fn held_labels(app: &mut App) -> usize {
        let mut query = app.world_mut().query::<&Held>();
        query.iter(app.world()).count()
    }

    #[test]
    fn pulling_out_moves_the_part_lets_the_rest_fall_and_settles_cleanly() {
        let directory = LocalTestDirectory::new();
        let model = model();
        let mut app = animated_app(&directory.0, LayoutInput::snapshot(self::model()));
        let before = objects(&mut app);
        let before_size = drawn(&mut app);
        let teaching = stage(&model);
        complete(
            &mut app,
            ViewerAction::Stage(Box::new(StagedScene {
                teaching: Some(teaching.clone()),
                transition: Some(Choreography::pull_out()),
                ..default()
            })),
        );
        assert_eq!(
            app.world().resource::<SceneTransition>().name(),
            Some("pull_out")
        );
        wait(&mut app, 0.0);
        let during = objects(&mut app);
        let during_size = drawn(&mut app);
        // The part travels from where it stood in the full model.
        for id in ["a", "c", "d"] {
            let (now, track) = &during[id];
            let track = track.as_ref().unwrap_or_else(|| panic!("{id} has no track"));
            assert_eq!(track.from, before[id].0, "{id} starts where it was");
            assert_eq!(*now, track.from, "{id} has not jumped");
            assert!(
                during_size[id].abs_diff_eq(before_size[id], 1e-4),
                "{id} starts at its old size: {} vs {}",
                during_size[id],
                before_size[id]
            );
            assert!(!track.despawn);
            assert!(track.glow.is_some(), "{id} lights up as it is lifted out");
        }
        // Only the members are in the new scene; the scopes and b fall away.
        for id in ["s0", "s1", "b"] {
            assert!(!during.contains_key(id), "{id} is still a scene object");
        }
        assert!(leaving(&mut app) >= 3, "{}", leaving(&mut app));
        assert!(held_labels(&mut app) > 0, "labels wait for the part to land");

        // Halfway: the part is between its two places.
        wait(&mut app, 0.0);
        wait(&mut app, Choreography::pull_out().travel.start + 0.5);
        let midway = objects(&mut app);
        let (now, track) = &midway["c"];
        let track = track.as_ref().unwrap();
        assert_ne!(track.from, track.to, "c moves to a new place");
        assert_ne!(*now, track.from);
        assert_ne!(*now, track.to);

        // Settled: every track is gone, the rest is removed, the part rests in place.
        wait(&mut app, Choreography::pull_out().duration() + 0.01);
        wait(&mut app, 0.0);
        assert!(!app.world().resource::<SceneTransition>().active());
        let after = objects(&mut app);
        for (id, (now, track)) in &after {
            assert!(track.is_none(), "{id} still has a track");
            if let Some((_, Some(track))) = during.get(id) {
                assert_eq!(*now, track.to, "{id} rests where its track ended");
            }
        }
        assert_eq!(leaving(&mut app), 0);
        assert_eq!(held_labels(&mut app), 0);
        assert_eq!(drawn(&mut app), own_size(&mut app), "settled at their own size");

        // Leaving the view puts the part back and the rest rises in around it.
        complete(&mut app, ViewerAction::LeaveTeaching);
        assert_eq!(
            app.world().resource::<SceneTransition>().name(),
            Some("put_back")
        );
        let returning = objects(&mut app);
        assert_eq!(returning["a"].1.as_ref().unwrap().from, after["a"].0);
        assert!(returning["s0"].1.as_ref().is_some_and(|t| !t.despawn));
        wait(&mut app, 0.0);
        wait(&mut app, Choreography::put_back().duration() + 0.01);
        wait(&mut app, 0.0);
        let settled = objects(&mut app);
        let moving: Vec<_> = settled
            .iter()
            .filter(|(_, (_, track))| track.is_some())
            .collect();
        assert!(moving.is_empty(), "{moving:?} {:?}", app.world().resource::<SceneTransition>().name());
        assert_eq!(settled["a"].0, before["a"].0);
    }

    #[test]
    fn a_member_that_changes_size_grows_or_shrinks_as_it_travels() {
        let directory = LocalTestDirectory::new();
        let model = model();
        // s1 holds c and d in the full model; lifted out alone it is empty and small.
        let view = TeachingView {
            id: "tv:scope".into(),
            title: "Scope".into(),
            members: vec!["a".into(), "s1".into()],
            layout: TeachingLayout::Pipeline,
        };
        let teaching = TeachingStage {
            input: LayoutInput::snapshot(teaching_model(&model, &view).unwrap()),
            policy: layout::projection::LayoutPolicy::Pipeline,
        };
        let mut app = animated_app(&directory.0, LayoutInput::snapshot(self::model()));
        let before = drawn(&mut app);
        complete(
            &mut app,
            ViewerAction::Stage(Box::new(StagedScene {
                teaching: Some(teaching),
                transition: Some(Choreography::pull_out()),
                ..default()
            })),
        );
        wait(&mut app, 0.0);
        let target = own_size(&mut app);
        assert!(
            !before["s1"].abs_diff_eq(target["s1"], 1e-3),
            "the fixture must resize s1: {} vs {}",
            before["s1"],
            target["s1"]
        );
        // It starts at its old size, not its new one.
        assert!(drawn(&mut app)["s1"].abs_diff_eq(before["s1"], 1e-3));
        // Midway it is between the two, on every axis that changes.
        let travel = Choreography::pull_out().travel;
        wait(&mut app, travel.start + travel.duration / 2.0);
        let midway = drawn(&mut app)["s1"];
        for axis in 0..3 {
            let (from, to, now) = (before["s1"][axis], target["s1"][axis], midway[axis]);
            if (from - to).abs() > 1e-3 {
                assert!(
                    now > from.min(to) && now < from.max(to),
                    "axis {axis}: {now} not between {from} and {to}"
                );
            }
        }
        // It settles at its own size, with no scale left over.
        wait(&mut app, Choreography::pull_out().duration() + 0.01);
        wait(&mut app, 0.0);
        assert!(drawn(&mut app)["s1"].abs_diff_eq(target["s1"], 1e-5));
    }

    #[test]
    fn a_scene_without_a_transition_still_cuts() {
        let directory = LocalTestDirectory::new();
        let model = model();
        let mut app = animated_app(&directory.0, LayoutInput::snapshot(self::model()));
        complete(&mut app, staged(Some(stage(&model))));
        assert!(!app.world().resource::<SceneTransition>().active());
        assert!(objects(&mut app).values().all(|(_, track)| track.is_none()));
        assert_eq!(leaving(&mut app), 0);
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
