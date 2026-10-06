mod click_isolation_tests {
    use super::*;
    use bevy::math::DVec3;
    use crate::layout_geometry::SceneBox;
    use layout::{Edge, Node as SceneNode};

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }
    fn click(
        press: Option<Vec2>,
        release: Vec2,
        hit: Option<&str>,
        last: &mut Option<LastClick>,
        now: u64,
    ) -> Click {
        classify_click(press, release, hit, last, at(now))
    }
    fn tap(hit: Option<&str>, last: &mut Option<LastClick>, now: u64) -> Click {
        let spot = Vec2::new(100.0, 100.0);
        click(Some(spot), spot, hit, last, now)
    }

    #[test]
    fn a_drag_past_the_threshold_is_not_a_click_and_breaks_a_pending_double_click() {
        let mut last = None;
        let press = Vec2::new(100.0, 100.0);
        assert_eq!(
            tap(Some("a"), &mut last, 0),
            Click::Single("a".into()),
            "primes a double click"
        );
        let far = press + Vec2::new(CLICK_DRAG_THRESHOLD, 0.0);
        assert_eq!(click(Some(press), far, Some("a"), &mut last, 50), Click::None);
        assert_eq!(last, None);
        assert_eq!(tap(Some("a"), &mut last, 100), Click::Single("a".into()));
        // Just under the threshold is still a click.
        let near = press + Vec2::new(CLICK_DRAG_THRESHOLD - 0.5, 0.0);
        assert_eq!(
            click(Some(press), near, Some("a"), &mut last, 150),
            Click::Double("a".into())
        );
    }
    #[test]
    fn a_press_that_did_not_start_on_the_scene_is_never_a_click() {
        let mut last = None;
        let spot = Vec2::new(5.0, 5.0);
        assert_eq!(click(None, spot, Some("a"), &mut last, 0), Click::None);
        assert_eq!(click(None, spot, None, &mut last, 10), Click::None);
        assert_eq!(last, None);
    }
    #[test]
    fn click_is_single_and_two_clicks_on_one_box_within_the_window_are_double() {
        let mut last = None;
        assert_eq!(tap(Some("a"), &mut last, 1000), Click::Single("a".into()));
        let edge = 1000 + DOUBLE_CLICK_WINDOW.as_millis() as u64;
        assert_eq!(tap(Some("a"), &mut last, edge), Click::Double("a".into()));
        // A pair is consumed: a third click starts over instead of re-entering.
        assert_eq!(tap(Some("a"), &mut last, edge + 10), Click::Single("a".into()));
    }
    #[test]
    fn clicks_on_different_boxes_or_after_the_window_are_two_singles() {
        let mut last = None;
        assert_eq!(tap(Some("a"), &mut last, 0), Click::Single("a".into()));
        assert_eq!(tap(Some("b"), &mut last, 100), Click::Single("b".into()));
        assert_eq!(tap(Some("a"), &mut last, 200), Click::Single("a".into()));
        let late = 200 + DOUBLE_CLICK_WINDOW.as_millis() as u64 + 1;
        assert_eq!(tap(Some("a"), &mut last, late), Click::Single("a".into()));
    }
    #[test]
    fn clicking_empty_space_clears_and_breaks_a_pending_double_click() {
        let mut last = None;
        assert_eq!(tap(Some("a"), &mut last, 0), Click::Single("a".into()));
        assert_eq!(tap(None, &mut last, 50), Click::Clear);
        assert_eq!(tap(Some("a"), &mut last, 100), Click::Single("a".into()));
    }

    #[test]
    fn single_click_selects_and_isolates_and_empty_click_clears_both() {
        let mut v = Viewer {
            selected: Some("old".into()),
            selected_relation: Some("rel".into()),
            relation_page: 3,
            ..default()
        };
        assert!(apply_click(&mut v, Click::Single("a".into())).is_none());
        assert!(v.isolate && v.dirty);
        assert_eq!(v.selected.as_deref(), Some("a"));
        assert_eq!((v.selected_relation.as_ref(), v.relation_page), (None, 0));
        // Moving isolation to another box.
        assert!(apply_click(&mut v, Click::Single("b".into())).is_none());
        assert!(v.isolate);
        assert_eq!(v.selected.as_deref(), Some("b"));
        v.dirty = false;
        assert!(apply_click(&mut v, Click::Clear).is_none());
        assert!(!v.isolate && v.dirty);
        assert_eq!((v.selected.as_ref(), v.selected_relation.as_ref()), (None, None));
        // Nothing to clear: no needless scene rebuild.
        v.dirty = false;
        assert!(apply_click(&mut v, Click::Clear).is_none());
        assert!(!v.dirty);
        assert!(apply_click(&mut v, Click::None).is_none());
        assert!(!v.dirty);
    }
    #[test]
    fn double_click_isolates_then_asks_to_enter_the_box() {
        let mut v = Viewer::default();
        assert!(matches!(
            apply_click(&mut v, Click::Double("a".into())),
            Some(ViewerAction::Enter(id)) if id == "a"
        ));
        assert!(v.isolate);
        assert_eq!(v.selected.as_deref(), Some("a"));
    }

    #[test]
    fn double_clicking_a_box_enters_it_and_leaves_isolation_behind() {
        let model = visualyze_model::architecture::Architecture::from_json(include_bytes!(
            "../../../public/examples/sketch-crud.json"
        ))
        .unwrap();
        let mut app = action_app(LayoutInput::snapshot(model), ViewState::default());
        let id = {
            let v = app.world().resource::<Viewer>();
            let (p, _) = v.accepted.as_ref().unwrap();
            p.nodes
                .iter()
                .find(|n| n.id != p.root && n.solid)
                .unwrap()
                .id
                .clone()
        };
        let mut last = None;
        let mut queue = Vec::new();
        for now in [0, 120] {
            let click = tap(Some(&id), &mut last, now);
            let mut v = app.world_mut().resource_mut::<Viewer>();
            queue.extend(apply_click(&mut v, click));
            if now == 0 {
                assert!(v.isolate && v.selected.as_deref() == Some(id.as_str()));
                assert!(queue.is_empty(), "the first click only isolates");
            }
        }
        assert!(matches!(queue.as_slice(), [ViewerAction::Enter(entered)] if *entered == id));
        complete(&mut app, queue.remove(0));
        let v = app.world().resource::<Viewer>();
        assert_eq!(v.view.focus.as_deref(), Some(id.as_str()));
        assert!(!v.isolate && v.selected.is_none());
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
    }

    #[test]
    fn backspace_leaves_isolation_before_the_navigation_trail() {
        let model = visualyze_model::architecture::Architecture::from_json(include_bytes!(
            "../../../public/examples/sketch-crud.json"
        ))
        .unwrap();
        let mut app = action_app(LayoutInput::snapshot(model), ViewState::default());
        let id = {
            let v = app.world().resource::<Viewer>();
            let (p, _) = v.accepted.as_ref().unwrap();
            p.nodes
                .iter()
                .find(|n| n.id != p.root && n.solid)
                .unwrap()
                .id
                .clone()
        };
        complete(&mut app, ViewerAction::Enter(id.clone()));
        {
            let mut v = app.world_mut().resource_mut::<Viewer>();
            let mut last = None;
            let click = tap(Some(&id), &mut last, 0);
            assert!(apply_click(&mut v, click).is_none());
            assert!(v.isolate);
        }
        complete(&mut app, ViewerAction::Back);
        let v = app.world().resource::<Viewer>();
        assert!(!v.isolate, "the first Backspace clears isolation");
        assert_eq!(v.view.focus.as_deref(), Some(id.as_str()), "and stays put");
        complete(&mut app, ViewerAction::Back);
        let v = app.world().resource::<Viewer>();
        assert_eq!(v.view.focus, None, "the next Backspace returns");
    }

    #[test]
    fn mouse_release_decides_the_click_and_a_drag_never_changes_isolation() {
        use bevy::input::mouse::AccumulatedMouseMotion;
        let mut app = App::new();
        app.insert_resource(Viewer {
            isolate: true,
            selected: Some("a".into()),
            ..default()
        })
        .init_resource::<ViewerActions>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<Time<Real>>()
        .init_resource::<AccumulatedMouseMotion>()
        .init_resource::<crate::camera::CameraInputLock>()
        .add_message::<MouseWheel>()
        .add_message::<KeyboardInput>()
        .add_systems(Update, controls);
        let mut window = Window {
            focused: true,
            resolution: bevy::window::WindowResolution::new(1280, 900),
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(300.0, 200.0)));
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        app.world_mut().spawn((
            Camera3d::default(),
            Camera::default(),
            GlobalTransform::default(),
        ));
        let step = |app: &mut App, cursor: Vec2, press: bool, release: bool| {
            app.world_mut()
                .get_mut::<Window>(window)
                .unwrap()
                .set_cursor_position(Some(cursor));
            let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
            mouse.clear();
            if press {
                mouse.press(MouseButton::Left);
            }
            if release {
                mouse.release(MouseButton::Left);
            }
            app.update();
        };
        let held = |app: &App| {
            let v = app.world().resource::<Viewer>();
            v.isolate && v.selected.as_deref() == Some("a")
        };
        // Press on a box, drag far away, release: selection and isolation stay.
        step(&mut app, Vec2::new(300.0, 200.0), true, false);
        step(&mut app, Vec2::new(340.0, 230.0), false, true);
        assert!(held(&app));
        // A press over open Details never becomes a scene click.
        app.world_mut().resource_mut::<Viewer>().inspector_expanded = true;
        app.world_mut().spawn((InspectorPanel, Node::default()));
        let details = details_rect(Vec2::new(1280.0, 900.0), true).center();
        step(&mut app, details, true, false);
        step(&mut app, details, false, true);
        assert!(held(&app));
        app.world_mut().resource_mut::<Viewer>().inspector_expanded = false;
        // A stationary press and release that hits no box clears isolation.
        step(&mut app, Vec2::new(300.0, 200.0), true, false);
        assert!(held(&app), "nothing happens on press");
        step(&mut app, Vec2::new(302.0, 201.0), false, true);
        let v = app.world().resource::<Viewer>();
        assert!(!v.isolate && v.selected.is_none());
    }

    fn node(id: &str, parent: &str, solid: bool) -> SceneNode {
        SceneNode {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: (!parent.is_empty()).then(|| parent.to_owned()),
            kind: if solid {
                NodeKind::Service
            } else {
                NodeKind::Group
            },
            band: 0,
            min_size: Point::new(12, 8, 12),
            preferred_size: Point::new(24, 16, 24),
            max_size: Point::new(48, 32, 48),
            solid,
        }
    }
    fn edge(id: &str, source: &str, target: &str) -> Edge {
        Edge {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            semantic_ids: vec![id.into()],
            bundle: None,
        }
    }
    /// root > F (> f1, f2, H) | G (> g1) | s | t, drawn in the x/y plane.
    fn frame_scene() -> (DesktopProjection, NativeLayout) {
        let (mut p, mut s) = solved();
        p.graph.root = "root".into();
        p.graph.nodes = vec![
            node("root", "", false),
            node("F", "root", false),
            node("f1", "F", true),
            node("f2", "F", true),
            node("H", "F", false),
            node("G", "root", false),
            node("g1", "G", true),
            node("s", "root", true),
            node("t", "root", true),
        ];
        p.graph.edges = vec![
            edge("e1", "f1", "g1"),
            edge("e2", "f1", "f2"),
            edge("e3", "s", "t"),
            edge("e4", "g1", "t"),
            edge("e5", "s", "f1"),
        ];
        let b = |x: (f64, f64), y: (f64, f64), z: f64, solid| SceneBox {
            min: DVec3::new(x.0, y.0, -z),
            max: DVec3::new(x.1, y.1, z),
            solid,
        };
        s.geometry.boxes = [
            ("root", b((-20.0, 20.0), (-10.0, 10.0), 5.0, false)),
            ("F", b((-12.0, -2.0), (-4.0, 4.0), 2.0, false)),
            ("f1", b((-11.0, -8.0), (-2.0, 2.0), 1.0, true)),
            ("f2", b((-6.0, -3.0), (-2.0, 2.0), 1.0, true)),
            ("H", b((-11.0, -8.0), (2.5, 3.8), 1.0, false)),
            ("G", b((2.0, 12.0), (-4.0, 4.0), 2.0, false)),
            ("g1", b((4.0, 8.0), (-2.0, 2.0), 1.0, true)),
            ("s", b((14.0, 18.0), (-2.0, 2.0), 1.0, true)),
            ("t", b((14.0, 18.0), (6.0, 9.0), 1.0, true)),
        ]
        .into_iter()
        .map(|(id, b)| (id.to_owned(), b))
        .collect();
        s.geometry.routes.clear();
        (p, s)
    }
    fn pick(v: &Viewer, x: f32, y: f32) -> Option<String> {
        let (p, s) = v.accepted.as_ref().unwrap();
        pick_box(v, p, s, Vec3::new(x, y, 50.0), -Vec3::Z)
    }
    fn scene_viewer() -> Viewer {
        let (p, s) = frame_scene();
        Viewer {
            accepted: Some((p, s)),
            ..default()
        }
    }

    #[test]
    fn picking_prefers_solid_boxes_then_the_innermost_frame_and_never_the_root() {
        let v = scene_viewer();
        // F is nearer the camera than f1, but the solid wins.
        assert_eq!(pick(&v, -9.5, 0.0).as_deref(), Some("f1"));
        // Only F, H and root are along this ray; the smaller frame wins.
        assert_eq!(pick(&v, -9.5, 3.0).as_deref(), Some("H"));
        // Only F and the root.
        assert_eq!(pick(&v, -5.0, 3.0).as_deref(), Some("F"));
        assert_eq!(pick(&v, 10.0, 3.0).as_deref(), Some("G"));
        // Only the focused root is under the ray.
        assert_eq!(pick(&v, 0.0, 0.0), None);
        assert_eq!(pick(&v, 100.0, 0.0), None);
    }
    #[test]
    fn picking_only_considers_what_isolation_still_shows() {
        let mut v = scene_viewer();
        v.isolate = true;
        v.selected = Some("s".into());
        assert_eq!(pick(&v, -9.5, 0.0).as_deref(), Some("f1"), "joined by e5");
        assert_eq!(pick(&v, -4.5, 0.0), None, "f2 is not");
        // s is a solid: its neighbours t and f1 stay (e3, e5), nothing else.
        assert_eq!(pick(&v, 16.0, 7.0).as_deref(), Some("t"));
        v.selected = Some("F".into());
        assert_eq!(pick(&v, -9.5, 3.0).as_deref(), Some("H"));
        assert_eq!(pick(&v, 16.0, 7.0), None, "t does not touch F");
        assert_eq!(pick(&v, 6.0, 0.0).as_deref(), Some("g1"));
    }

    fn shown(v: &Viewer) -> Vec<String> {
        let (p, _) = v.accepted.as_ref().unwrap();
        p.nodes
            .iter()
            .filter(|n| visible_id(v, p, &n.id))
            .map(|n| n.id.clone())
            .collect()
    }
    fn lines(v: &Viewer) -> Vec<String> {
        let (p, _) = v.accepted.as_ref().unwrap();
        p.edges
            .iter()
            .filter(|e| visible_edge(v, p, e))
            .map(|e| e.id.clone())
            .collect()
    }
    #[test]
    fn isolating_a_solid_box_is_unchanged() {
        let mut v = scene_viewer();
        v.isolate = true;
        v.selected = Some("s".into());
        assert_eq!(shown(&v), ["f1", "s", "t"]);
        assert_eq!(lines(&v), ["e3", "e5"]);
        v.selected = Some("g1".into());
        assert_eq!(shown(&v), ["f1", "g1", "t"]);
        assert_eq!(lines(&v), ["e1", "e4"]);
        v.isolate = false;
        assert_eq!(shown(&v).len(), 9);
        assert_eq!(lines(&v).len(), 5);
    }
    #[test]
    fn isolating_an_opened_box_keeps_its_contents_and_every_line_touching_them() {
        let mut v = scene_viewer();
        v.isolate = true;
        v.selected = Some("F".into());
        // F's contents, plus the far ends of e1 (g1, inside G) and e5 (s). G
        // stays so e1 does not end in a hidden container; the focused root and
        // unrelated boxes (t) do not.
        assert_eq!(shown(&v), ["F", "f1", "f2", "H", "G", "g1", "s"]);
        assert_eq!(lines(&v), ["e1", "e2", "e5"]);
        v.selected = Some("G".into());
        // g1's lines: e1 reaches f1 inside F, so F stays; e4 reaches t.
        assert_eq!(shown(&v), ["F", "f1", "G", "g1", "t"]);
        assert_eq!(lines(&v), ["e1", "e4"]);
        // A path selection still wins over a frame selection.
        v.selected_relation = Some("e3".into());
        assert_eq!(lines(&v), ["e3"]);
    }
    #[test]
    fn an_unopened_box_with_no_displayed_contents_isolates_as_a_plain_box() {
        let mut v = scene_viewer();
        v.isolate = true;
        // H is a frame with nothing displayed inside it and no lines.
        v.selected = Some("H".into());
        assert_eq!(shown(&v), ["H"]);
        assert!(lines(&v).is_empty());
    }
}
