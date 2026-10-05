use std::collections::BTreeMap;
use visualyze_layout::{
    Bounds, LAYOUT_VERSION, LayoutBox, LayoutSolution, Point, Route, ScopePort,
    assembly::{
        AnchorPin, Assembly, AssemblyError, AssemblyInstance, CanonicalFrame, DyadicTransform,
        ExactBounds, ExactPoint, MAX_COORDINATE, MAX_SHIFT,
    },
};

fn p(x: i64, y: i64, z: i64) -> Point {
    Point::new(x, y, z)
}
fn b(min: Point, max: Point) -> Bounds {
    Bounds { min, max }
}
fn cube(min: i64, max: i64) -> Bounds {
    b(p(min, min, min), p(max, max, max))
}
fn object(id: &str, parent: Option<&str>, bounds: Bounds, solid: bool) -> LayoutBox {
    LayoutBox {
        id: id.into(),
        semantic_ids: vec![id.into()],
        parent: parent.map(str::to_owned),
        bounds,
        solid,
    }
}
fn solution(root: &str, bounds: Bounds, children: &[(&str, Bounds)]) -> LayoutSolution {
    let mut boxes = BTreeMap::from([(root.into(), object(root, None, bounds, false))]);
    boxes.extend(
        children
            .iter()
            .map(|(id, bounds)| (String::from(*id), object(id, Some(root), *bounds, true))),
    );
    LayoutSolution {
        model_id: "canonical-test".into(),
        revision: 1,
        projection_key: format!("frame:{root}"),
        version: LAYOUT_VERSION,
        boxes,
        routes: vec![],
        metrics: Default::default(),
        diagnostics: vec![],
    }
}
fn canonical() -> Vec<LayoutSolution> {
    let root = solution("root", cube(0, 200), &[("middle", cube(10, 90))]);
    let middle = solution("middle", cube(0, 160), &[("leaf", cube(20, 100))]);
    let mut leaf = solution(
        "leaf",
        cube(0, 160),
        &[("a", cube(0, 16)), ("b", b(p(80, 0, 0), p(96, 16, 16)))],
    );
    leaf.boxes.insert(
        "inner-scope".into(),
        object(
            "inner-scope",
            Some("leaf"),
            b(p(0, 0, 0), p(40, 20, 20)),
            false,
        ),
    );
    leaf.boxes.get_mut("a").unwrap().parent = Some("inner-scope".into());
    leaf.routes.push(Route {
        edge_id: "a-to-b".into(),
        semantic_ids: vec!["supplied:relationship".into()],
        points: vec![p(16, 8, 8), p(80, 8, 8)],
        scope_ports: vec![ScopePort {
            assembly_id: "inner-scope".into(),
            point: p(40, 8, 8),
            entering: false,
        }],
    });
    vec![root, middle, leaf]
}
fn instances() -> Vec<AssemblyInstance> {
    vec![
        AssemblyInstance::root("root"),
        AssemblyInstance::child(
            "middle",
            "root",
            DyadicTransform::new(Point::ZERO, p(10, 10, 10), 1).unwrap(),
        ),
        AssemblyInstance::child(
            "leaf",
            "middle",
            DyadicTransform::new(Point::ZERO, p(20, 20, 20), 1).unwrap(),
        ),
    ]
}
fn assembly(
    solutions: Vec<LayoutSolution>,
    instances: Vec<AssemblyInstance>,
) -> Result<Assembly, AssemblyError> {
    Assembly::new(
        solutions
            .into_iter()
            .map(CanonicalFrame::new)
            .collect::<Result<_, _>>()?,
        instances,
    )
}

#[test]
fn two_level_construction_presents_every_geometry_kind_through_one_exact_map() {
    // This is the construction path for integration: validate/solve each local
    // projection, wrap its unchanged solution, then attach it to its host box.
    let tree = assembly(canonical(), instances()).unwrap();
    let map = tree.transform_to("leaf", "root").unwrap();
    assert_eq!(map.scale_shift(), 2);
    assert_eq!(
        map.map_point(p(1, 2, 3)).unwrap(),
        ExactPoint::new([81, 82, 83], 2).unwrap()
    );
    let presented = tree.present("leaf", "root").unwrap();
    assert_eq!(
        presented.boxes["a"].bounds.min.to_integer().unwrap(),
        p(20, 20, 20)
    );
    assert_eq!(
        presented.boxes["a"].bounds.max.to_integer().unwrap(),
        p(24, 24, 24)
    );
    assert_eq!(
        presented.routes[0].points[0],
        map.map_point(p(16, 8, 8)).unwrap()
    );
    assert_eq!(
        presented.routes[0].scope_ports[0].point,
        map.map_point(p(40, 8, 8)).unwrap()
    );
    assert_eq!(presented.routes[0].semantic_ids, ["supplied:relationship"]);
    assert!(!tree.present("root", "root").unwrap().boxes["middle"].solid);
    assert_eq!(
        tree.present("leaf", "leaf").unwrap().boxes["a"].bounds,
        ExactBounds::integer(cube(0, 16)).unwrap()
    );
    assert_eq!(
        map.inverse_integer(map.map_point(p(1, 2, 3)).unwrap())
            .unwrap(),
        p(1, 2, 3)
    );
    assert_eq!(
        map.map_point(p(1, 2, 3)).unwrap().to_f64(),
        [20.25, 20.5, 20.75]
    );
}

#[test]
fn boundary_terminals_are_exact_or_rejected_without_rounding() {
    let map = DyadicTransform::new(p(4, 8, 12), p(10, 20, 30), 2).unwrap();
    let bounds = b(p(4, 8, 12), p(44, 48, 52));
    let terminal = p(44, 20, 28);
    let parent = map.map_boundary_terminal(terminal, bounds).unwrap();
    assert_eq!(parent, p(20, 23, 34));
    assert_eq!(
        map.inverse_boundary_terminal(parent, bounds).unwrap(),
        terminal
    );
    assert_eq!(
        map.map_boundary_terminal(p(44, 21, 28), bounds),
        Err(AssemblyError::NonIntegral)
    );
    assert_eq!(
        map.inverse_boundary_terminal(p(15, 25, 35), bounds),
        Err(AssemblyError::NotBoundary)
    );
    let outer = DyadicTransform::new(Point::ZERO, Point::ZERO, 1).unwrap();
    let inner = DyadicTransform::new(Point::ZERO, p(1, 1, 1), 1).unwrap();
    let composed = outer.compose(inner).unwrap();
    assert_eq!(
        composed.translation(),
        ExactPoint::new([1, 1, 1], 1).unwrap()
    );
    assert_eq!(
        composed
            .inverse_integer(ExactPoint::integer(p(2, 2, 2)).unwrap())
            .unwrap(),
        p(6, 6, 6)
    );
    assert_eq!(
        DyadicTransform::identity().inverse_integer(ExactPoint::new([1, 2, 3], 1).unwrap()),
        Err(AssemblyError::NonIntegral)
    );
}

#[test]
fn fit_uses_bounded_uniform_scale_and_discloses_padding() {
    let local = b(p(-80, -40, 20), p(80, 40, 60));
    let envelope = cube(10, 30);
    let map = DyadicTransform::fit(local, envelope).unwrap();
    assert_eq!(map.scale_shift(), 3);
    assert_eq!(map.origin(), local.min);
    let presented = map.map_bounds(local).unwrap();
    assert_eq!(presented.min.to_integer().unwrap(), envelope.min);
    assert_eq!(presented.max.to_integer().unwrap(), p(30, 20, 15));
    assert!(ExactBounds::integer(envelope).unwrap().contains(presented));
    assert_eq!(
        map.inverse_boundary_terminal(p(20, 30, 20), local),
        Err(AssemblyError::NotBoundary)
    );
    assert!(DyadicTransform::fit(cube(0, MAX_COORDINATE), cube(0, 1)).is_err());
}

#[test]
fn exact_envelope_fit_maps_all_six_parent_faces_with_odd_sizes_and_negative_origin() {
    let local = b(p(-83, -41, -19), p(80, 40, 61));
    let envelope = b(p(-15, 7, -33), p(6, 20, -16));
    let fit = DyadicTransform::fit_envelope(local, envelope).unwrap();
    assert_eq!(fit.transform.scale_shift(), 3);
    assert_eq!(fit.canonical_bounds.min, local.min);
    assert_eq!(fit.canonical_bounds.max, p(85, 63, 117));
    assert!(fit.canonical_bounds.contains_point(local.max));
    assert_eq!(
        fit.transform.map_bounds(fit.canonical_bounds).unwrap(),
        ExactBounds::integer(envelope).unwrap()
    );
    let inside = p(-7, 12, -25);
    for axis in 0..3 {
        for maximum in [false, true] {
            let mut parent = [inside.x, inside.y, inside.z];
            let face = if maximum { envelope.max } else { envelope.min };
            parent[axis] = [face.x, face.y, face.z][axis];
            let parent = p(parent[0], parent[1], parent[2]);
            let terminal = fit
                .transform
                .inverse_boundary_terminal(parent, fit.canonical_bounds)
                .unwrap();
            assert_eq!(
                fit.transform
                    .map_boundary_terminal(terminal, fit.canonical_bounds)
                    .unwrap(),
                parent
            );
        }
    }
    assert!(
        DyadicTransform::fit_envelope(cube(MAX_COORDINATE - 1, MAX_COORDINATE), cube(0, 2))
            .is_err()
    );
}

#[test]
fn top_frame_forest_preserves_solid_focus_interfaces_and_reference_parents() {
    let mut s = solution("focus", cube(0, 20), &[]);
    s.boxes.get_mut("focus").unwrap().solid = true;
    s.boxes.insert(
        "interface".into(),
        object(
            "interface",
            Some("focus"),
            b(p(2, 2, 20), p(8, 8, 24)),
            true,
        ),
    );
    s.boxes.insert(
        "reference".into(),
        object("reference", None, b(p(40, 0, 0), p(50, 10, 10)), true),
    );
    let original = s.boxes.clone();
    assert!(CanonicalFrame::new(s.clone()).is_err());
    let frame = CanonicalFrame::with_root("focus", b(p(0, 0, 0), p(50, 20, 24)), s).unwrap();
    let tree = Assembly::new(vec![frame], vec![AssemblyInstance::root("focus")]).unwrap();
    let shown = tree.present("focus", "focus").unwrap();
    for (id, object) in original {
        assert_eq!(shown.boxes[&id].parent, object.parent);
        assert_eq!(
            shown.boxes[&id].bounds,
            ExactBounds::integer(object.bounds).unwrap()
        );
    }
    assert!(shown.boxes["focus"].solid);
    assert_eq!(shown.boxes.len(), 3);
}

#[test]
fn interface_protrusion_stays_inside_every_open_ancestor_in_both_geometry_gates() {
    use visualyze_layout::{Node, NodeKind, Projection, validate};

    for pad_max_z in [49, 55] {
        let mut solved = solution("root", cube(0, 100), &[]);
        solved.boxes.extend([
            (
                "group".into(),
                object("group", Some("root"), cube(0, 50), false),
            ),
            (
                "service".into(),
                object(
                    "service",
                    Some("group"),
                    b(p(10, 10, 10), p(30, 30, 45)),
                    true,
                ),
            ),
            (
                "pad".into(),
                object(
                    "pad",
                    Some("service"),
                    b(p(12, 12, 45), p(20, 20, pad_max_z)),
                    true,
                ),
            ),
        ]);
        let projection = Projection {
            key: solved.projection_key.clone(),
            model_id: solved.model_id.clone(),
            revision: solved.revision,
            root: "root".into(),
            nodes: solved
                .boxes
                .values()
                .map(|object| Node {
                    id: object.id.clone(),
                    semantic_ids: object.semantic_ids.clone(),
                    parent: object.parent.clone(),
                    kind: match object.id.as_str() {
                        "pad" => NodeKind::Interface,
                        "service" => NodeKind::Service,
                        _ => NodeKind::Scope,
                    },
                    band: 0,
                    min_size: object.bounds.size(),
                    max_size: object.bounds.size(),
                    preferred_size: object.bounds.size(),
                    solid: object.solid,
                })
                .collect(),
            edges: vec![],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        };
        let ordinary = validate(&projection, &solved, &BTreeMap::new(), 2);
        let canonical = CanonicalFrame::new(solved);
        if pad_max_z < 50 {
            ordinary.unwrap();
            canonical.unwrap();
        } else {
            assert!(
                ordinary
                    .unwrap_err()
                    .to_string()
                    .contains("child outside parent: pad")
            );
            assert!(
                canonical
                    .unwrap_err()
                    .to_string()
                    .contains("invalid canonical containment")
            );
        }
    }
}

#[test]
fn parentless_visual_assembly_is_hosted_without_inventing_semantic_parent() {
    let mut top = solution("focus", cube(0, 20), &[]);
    top.boxes.get_mut("focus").unwrap().solid = true;
    top.boxes.insert(
        "outside-group".into(),
        object("outside-group", None, b(p(40, 0, 0), p(50, 10, 10)), true),
    );
    let child = solution(
        "outside-group",
        cube(-10, 10),
        &[("outside-item", cube(-5, 5))],
    );
    let top = CanonicalFrame::with_root("focus", b(p(0, 0, 0), p(50, 20, 20)), top).unwrap();
    let child = CanonicalFrame::new(child).unwrap();
    let tree = Assembly::new(
        vec![top, child],
        vec![
            AssemblyInstance::root("focus"),
            AssemblyInstance::child(
                "outside-group",
                "focus",
                DyadicTransform::new(p(-10, -10, -10), p(40, 0, 0), 1).unwrap(),
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        tree.present("focus", "focus").unwrap().boxes["outside-group"].parent,
        None
    );
    assert_eq!(
        tree.present("outside-group", "focus").unwrap().boxes["outside-item"]
            .bounds
            .min
            .to_f64(),
        [42.5, 2.5, 2.5]
    );
}

#[test]
fn unrelated_duplicate_ids_fail_but_explicit_reference_semantic_members_may_repeat() {
    let mut solutions = canonical();
    solutions[0]
        .boxes
        .insert("a".into(), object("a", Some("root"), cube(110, 126), true));
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    let mut reference = object("reference-to-a", Some("root"), cube(110, 126), true);
    reference.semantic_ids = vec!["a".into()];
    solutions[0].boxes.insert(reference.id.clone(), reference);
    assembly(solutions, instances()).unwrap();
}

#[test]
fn real_solved_notes_geometry_and_interface_attachments_remain_accepted() {
    use std::sync::atomic::AtomicBool;
    use visualyze_layout::{
        Algorithm, LayoutRequest, QualityProfile,
        projection::{ViewState, project},
        solve, validate,
    };
    use visualyze_model::architecture::Architecture;
    let model = Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
        .unwrap();
    for focus in [None, Some("service:notes")] {
        let mut view = ViewState::default();
        view.set_expanded("service:notes", true);
        if let Some(focus) = focus {
            view.enter(focus);
        }
        let projection = project(&model, &view).unwrap();
        let solved = solve(LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        validate(&projection, &solved, &BTreeMap::new(), 2).unwrap();
        let mut domain = solved
            .boxes
            .values()
            .map(|object| object.bounds)
            .reduce(|a, b| Bounds {
                min: a.min.min(b.min),
                max: a.max.max(b.max),
            })
            .unwrap();
        for route in &solved.routes {
            for point in &route.points {
                domain.min = domain.min.min(*point);
                domain.max = domain.max.max(*point);
            }
        }
        let frame = CanonicalFrame::with_root(&projection.root, domain, solved.clone()).unwrap();
        let tree =
            Assembly::new(vec![frame], vec![AssemblyInstance::root(&projection.root)]).unwrap();
        assert_eq!(
            tree.frame(&projection.root).unwrap().solution().boxes,
            solved.boxes
        );
        assert_eq!(
            tree.present(&projection.root, &projection.root)
                .unwrap()
                .routes
                .len(),
            solved.routes.len()
        );
        assert!(
            projection
                .nodes
                .iter()
                .any(|node| node.kind == visualyze_layout::NodeKind::Interface)
        );
    }
}

#[test]
fn malformed_numeric_inputs_fail_before_existing_point_arithmetic() {
    assert_eq!(
        ExactPoint::new([i128::MIN, 0, 0], 0),
        Err(AssemblyError::Overflow)
    );
    assert_eq!(
        ExactPoint::new([0, 0, 0], 255),
        Err(AssemblyError::Limit("dyadic denominator"))
    );
    assert_eq!(
        DyadicTransform::new(Point::ZERO, Point::ZERO, MAX_SHIFT + 1),
        Err(AssemblyError::Limit("transform scale"))
    );
    let map = DyadicTransform::new(Point::ZERO, p(MAX_COORDINATE, 0, 0), 0).unwrap();
    assert_eq!(map.map_point(p(1, 0, 0)), Err(AssemblyError::Overflow));
    let mut solutions = canonical();
    solutions[0].boxes.get_mut("root").unwrap().bounds = cube(i64::MIN, i64::MAX);
    assert!(matches!(
        assembly(solutions, instances()),
        Err(AssemblyError::Overflow)
    ));
    assert!(CanonicalFrame::new(solution("root", cube(10, 0), &[])).is_err());
    let shrink = DyadicTransform::new(Point::ZERO, Point::ZERO, MAX_SHIFT).unwrap();
    assert!(
        shrink
            .compose(DyadicTransform::new(Point::ZERO, Point::ZERO, 1).unwrap())
            .is_err()
    );
}

#[test]
fn composition_rejects_invalid_ownership_semantics_revision_and_fit() {
    let mut links = instances();
    links[1].parent = None;
    assert!(assembly(canonical(), links).is_err());
    let mut links = instances();
    links[0].transform = DyadicTransform::new(Point::ZERO, p(1, 0, 0), 0).unwrap();
    assert!(assembly(canonical(), links).is_err());
    let mut links = instances();
    links[1].parent = Some("absent".into());
    assert!(assembly(canonical(), links).is_err());
    let mut links = instances();
    links[2] = links[1].clone();
    assert!(assembly(canonical(), links).is_err());
    let mut solutions = canonical();
    solutions[1].revision = 2;
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    solutions[1].model_id = "different-model".into();
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    solutions[1].boxes.get_mut("middle").unwrap().semantic_ids = vec!["invented".into()];
    assert!(assembly(solutions, instances()).is_err());
    let mut links = instances();
    links[1].transform = DyadicTransform::new(Point::ZERO, p(20, 20, 20), 0).unwrap();
    assert!(assembly(canonical(), links).is_err());
    let tree = assembly(canonical(), instances()).unwrap();
    assert!(tree.transform_to("root", "leaf").is_err());
    assert!(tree.present("leaf", "absent").is_err());
}

#[test]
fn cycles_missing_parents_and_geometry_shape_are_rejected() {
    let mut solutions = canonical();
    solutions[1].boxes.get_mut("leaf").unwrap().parent = Some("leaf".into());
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    solutions[1].boxes.get_mut("leaf").unwrap().parent = Some("missing".into());
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    solutions[2].routes[0].points[1] = p(80, 9, 8);
    assert!(assembly(solutions, instances()).is_err());
    let mut solutions = canonical();
    solutions[2].routes[0].points[1] = p(161, 8, 8);
    assert!(assembly(solutions, instances()).is_err());
    // A disconnected cycle has valid host boxes but cannot reach the root.
    let solutions = vec![
        solution("root", cube(0, 100), &[]),
        solution("one", cube(0, 100), &[("two", cube(0, 100))]),
        solution("two", cube(0, 100), &[("one", cube(0, 100))]),
    ];
    let links = vec![
        AssemblyInstance::root("root"),
        AssemblyInstance::child("one", "two", DyadicTransform::identity()),
        AssemblyInstance::child("two", "one", DyadicTransform::identity()),
    ];
    assert!(assembly(solutions, links).is_err());
}

#[test]
fn root_pin_locks_every_translation_scale_origin_and_parent_envelope() {
    let tree = assembly(canonical(), instances()).unwrap();
    let pin = AnchorPin::capture(&tree, "leaf", "root", "a").unwrap();
    assert_eq!(pin.anchored_point().to_integer().unwrap(), p(22, 22, 22));
    pin.validate(&tree).unwrap();
    for index in [1, 2] {
        for mutation in 0..3 {
            let mut links = instances();
            let mut solutions = canonical();
            let frame_id = if index == 1 { "middle" } else { "leaf" };
            let translation = if index == 1 { 10 } else { 20 };
            match mutation {
                0 => {
                    solutions[index - 1].boxes.get_mut(frame_id).unwrap().bounds =
                        cube(translation + 1, translation + 81);
                    links[index].transform = DyadicTransform::new(
                        Point::ZERO,
                        p(translation + 1, translation + 1, translation + 1),
                        1,
                    )
                    .unwrap();
                }
                1 => {
                    solutions[index].boxes.get_mut(frame_id).unwrap().bounds = cube(0, 320);
                    links[index].transform = DyadicTransform::new(
                        Point::ZERO,
                        p(translation, translation, translation),
                        2,
                    )
                    .unwrap();
                }
                _ => {
                    // Only the origin/translation representation changes.
                    // Canonical domain, envelope and every presented point are
                    // unchanged, but an explicit frame lock still rejects it.
                    links[index].transform = DyadicTransform::new(
                        p(2, 2, 2),
                        p(translation + 1, translation + 1, translation + 1),
                        1,
                    )
                    .unwrap();
                }
            }
            let changed = assembly(solutions, links).unwrap();
            if mutation == 2 {
                assert_eq!(
                    tree.transform_to("leaf", "root")
                        .unwrap()
                        .map_point(p(8, 8, 8))
                        .unwrap(),
                    changed
                        .transform_to("leaf", "root")
                        .unwrap()
                        .map_point(p(8, 8, 8))
                        .unwrap()
                );
            }
            assert!(matches!(
                pin.validate(&changed),
                Err(AssemblyError::PinConflict(_))
            ));
        }
    }
    let mut solutions = canonical();
    // Enlarge the envelope and matching canonical root while leaving its origin
    // and affine map unchanged. The stored envelope/domain lock still rejects it.
    solutions[0].boxes.get_mut("middle").unwrap().bounds = cube(10, 100);
    solutions[1].boxes.get_mut("middle").unwrap().bounds = cube(0, 180);
    let changed = assembly(solutions, instances()).unwrap();
    assert!(matches!(
        pin.validate(&changed),
        Err(AssemblyError::PinConflict(_))
    ));
    let mut solutions = canonical();
    solutions[2].boxes.get_mut("a").unwrap().bounds = cube(1, 17);
    let changed = assembly(solutions, instances()).unwrap();
    assert!(matches!(
        pin.validate(&changed),
        Err(AssemblyError::PinConflict(_))
    ));
}

#[test]
fn local_pin_allows_ancestor_motion_but_keeps_object_center_and_identity() {
    let tree = assembly(canonical(), instances()).unwrap();
    let pin = AnchorPin::capture(&tree, "leaf", "leaf", "a").unwrap();
    let mid_pin = AnchorPin::capture(&tree, "leaf", "middle", "a").unwrap();
    let mut solutions = canonical();
    let mut links = instances();
    solutions[0].boxes.get_mut("middle").unwrap().bounds = cube(20, 100);
    links[1].transform = DyadicTransform::new(Point::ZERO, p(20, 20, 20), 1).unwrap();
    let changed = assembly(solutions, links).unwrap();
    pin.validate(&changed).unwrap();
    mid_pin.validate(&changed).unwrap();
    let mut links = instances();
    let mut solutions = canonical();
    solutions[1].boxes.get_mut("leaf").unwrap().bounds = cube(30, 110);
    links[2].transform = DyadicTransform::new(Point::ZERO, p(30, 30, 30), 1).unwrap();
    let changed = assembly(solutions, links).unwrap();
    pin.validate(&changed).unwrap();
    assert!(mid_pin.validate(&changed).is_err());
    let mut solutions = canonical();
    solutions[2].boxes.get_mut("a").unwrap().semantic_ids = vec!["replacement".into()];
    assert!(
        pin.validate(&assembly(solutions, instances()).unwrap())
            .is_err()
    );
}

#[test]
fn ordering_and_exact_normalization_are_deterministic() {
    assert_eq!(
        ExactPoint::new([2, 4, 6], 2).unwrap(),
        ExactPoint::new([1, 2, 3], 1).unwrap()
    );
    let a = assembly(canonical(), instances()).unwrap();
    let mut solutions = canonical();
    solutions.reverse();
    let mut links = instances();
    links.reverse();
    let b = assembly(solutions, links).unwrap();
    assert_eq!(
        a.present("leaf", "root").unwrap(),
        b.present("leaf", "root").unwrap()
    );
    assert_eq!(
        AnchorPin::capture(&a, "leaf", "root", "a").unwrap(),
        AnchorPin::capture(&b, "leaf", "root", "a").unwrap()
    );
}

#[test]
fn composition_and_inverse_agree_over_signed_points_and_dyadic_scales() {
    for outer_shift in 0..=4 {
        for inner_shift in 0..=4 {
            let outer = DyadicTransform::new(p(-17, 8, -3), p(5, -8, 11), outer_shift).unwrap();
            let inner = DyadicTransform::new(p(2, -9, 13), p(-7, 3, -5), inner_shift).unwrap();
            let composed = outer.compose(inner).unwrap();
            for value in -32..=32 {
                let local = p(value, 2 * value - 1, -value + 3);
                let two_steps = outer.map_exact(inner.map_point(local).unwrap()).unwrap();
                assert_eq!(composed.map_point(local).unwrap(), two_steps);
                assert_eq!(composed.inverse_integer(two_steps).unwrap(), local);
            }
        }
    }
}

#[test]
fn one_link_transform_preserves_its_origin_between_opposite_coordinate_limits() {
    let local = cube(MAX_COORDINATE - 10, MAX_COORDINATE);
    let envelope = cube(-MAX_COORDINATE, -MAX_COORDINATE + 10);
    let transform = DyadicTransform::new(local.min, envelope.min, 0).unwrap();
    assert_eq!(
        transform.map_bounds(local).unwrap(),
        ExactBounds::integer(envelope).unwrap()
    );
    assert_eq!(
        transform.compose(DyadicTransform::identity()),
        Ok(transform)
    );
    assert_eq!(
        DyadicTransform::identity().compose(transform),
        Ok(transform)
    );
    let build = |transform| {
        assembly(
            vec![
                solution("root", envelope, &[("child", envelope)]),
                solution("child", local, &[]),
            ],
            vec![
                AssemblyInstance::root("root"),
                AssemblyInstance::child("child", "root", transform),
            ],
        )
        .unwrap()
    };
    let tree = build(transform);
    assert_eq!(tree.transform_to("child", "root").unwrap(), transform);
    assert_eq!(
        tree.transform_to("child", "child").unwrap(),
        DyadicTransform::identity()
    );
    let pin = AnchorPin::capture(&tree, "child", "root", "child").unwrap();
    assert_eq!(
        pin.anchored_point().to_integer().unwrap(),
        envelope.center()
    );
    pin.validate(&tree).unwrap();
    // The affine map stays the same, but the explicit frame-origin lock still
    // rejects a changed transform representation after the identity fix.
    let shifted =
        DyadicTransform::new(local.min + p(1, 1, 1), envelope.min + p(1, 1, 1), 0).unwrap();
    assert_eq!(shifted.map_bounds(local), transform.map_bounds(local));
    assert!(matches!(
        pin.validate(&build(shifted)),
        Err(AssemblyError::PinConflict(_))
    ));
}

#[test]
fn instance_depth_and_accumulated_shrink_are_bounded_independently() {
    let mut solutions = Vec::new();
    let mut links = Vec::new();
    for index in 0..=33 {
        let id = format!("frame:{index}");
        let child = format!("frame:{}", index + 1);
        let children = [(child.as_str(), cube(0, 100))];
        solutions.push(solution(
            &id,
            cube(0, 100),
            if index == 33 { &[] } else { &children },
        ));
        links.push(if index == 0 {
            AssemblyInstance::root(id)
        } else {
            AssemblyInstance::child(
                id,
                format!("frame:{}", index - 1),
                DyadicTransform::identity(),
            )
        });
    }
    assert_eq!(
        assembly(solutions, links).unwrap_err(),
        AssemblyError::Limit("instance depth")
    );
    let width = 1 << MAX_SHIFT;
    let solutions = vec![
        solution("root", cube(0, width), &[("middle", cube(0, 1))]),
        solution("middle", cube(0, width), &[("leaf", cube(0, 1))]),
        solution("leaf", cube(0, width), &[]),
    ];
    let links = vec![
        AssemblyInstance::root("root"),
        AssemblyInstance::child(
            "middle",
            "root",
            DyadicTransform::new(Point::ZERO, Point::ZERO, MAX_SHIFT).unwrap(),
        ),
        AssemblyInstance::child(
            "leaf",
            "middle",
            DyadicTransform::new(Point::ZERO, Point::ZERO, MAX_SHIFT).unwrap(),
        ),
    ];
    assert_eq!(
        assembly(solutions, links).unwrap_err(),
        AssemblyError::Limit("composed scale")
    );
}
