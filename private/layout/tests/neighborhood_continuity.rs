use std::collections::BTreeMap;
use visualyze_layout::{
    Bounds, Edge, LAYOUT_VERSION, LayoutBox, LayoutSolution, Metrics, NeighborhoodComparisonStatus,
    NeighborhoodContinuity, Node, NodeKind, Point, Projection, Route, measure, validate,
};

fn add_box(
    solution: &mut LayoutSolution,
    id: &str,
    parent: Option<&str>,
    center: Point,
    solid: bool,
) {
    let half = if parent.is_none() {
        Point::new(1000, 1000, 1000)
    } else if solid {
        Point::new(2, 2, 2)
    } else {
        Point::new(50, 10, 10)
    };
    solution.boxes.insert(
        id.into(),
        LayoutBox {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            bounds: Bounds {
                min: center - half,
                max: center + half,
            },
            solid,
        },
    );
}

fn fixture() -> LayoutSolution {
    let mut solution = LayoutSolution {
        model_id: "continuity".into(),
        revision: 1,
        projection_key: "continuity".into(),
        version: LAYOUT_VERSION,
        boxes: BTreeMap::new(),
        routes: vec![],
        metrics: Metrics::default(),
        diagnostics: vec![],
    };
    add_box(&mut solution, "root", None, Point::ZERO, false);
    for (id, x) in [
        ("a", 0),
        ("b", 10),
        ("c", 20),
        ("d", 100),
        ("e", 110),
        ("f", 120),
    ] {
        add_box(&mut solution, id, Some("root"), Point::new(x, 0, 0), true);
    }
    solution
}

fn projection(solution: &LayoutSolution) -> Projection {
    Projection {
        key: solution.projection_key.clone(),
        model_id: solution.model_id.clone(),
        revision: solution.revision,
        root: solution
            .boxes
            .values()
            .find(|b| b.parent.is_none())
            .unwrap()
            .id
            .clone(),
        nodes: solution
            .boxes
            .values()
            .map(|b| Node {
                id: b.id.clone(),
                semantic_ids: b.semantic_ids.clone(),
                parent: b.parent.clone(),
                kind: if b.solid {
                    NodeKind::Service
                } else {
                    NodeKind::Scope
                },
                band: 0,
                min_size: b.bounds.size(),
                max_size: b.bounds.size(),
                preferred_size: b.bounds.size(),
                solid: b.solid,
            })
            .collect(),
        edges: vec![],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    }
}

fn continuity(
    current: &LayoutSolution,
    previous: Option<&LayoutSolution>,
) -> NeighborhoodContinuity {
    let p = projection(current);
    validate(&p, current, &BTreeMap::new(), 2).unwrap();
    let metrics = measure(&p, current, previous);
    assert_eq!(
        metrics.structural_cost,
        measure(&p, current, None).structural_cost,
        "continuity must not change the structural score"
    );
    metrics.neighborhood
}

#[test]
fn coherent_translation_retains_neighbors_while_permutation_disrupts_them() {
    let old = fixture();
    let mut translated = old.clone();
    for b in translated.boxes.values_mut() {
        b.bounds = b.bounds.translated(Point::new(30, -80, 120));
    }
    let measured = continuity(&translated, Some(&old));
    assert_eq!(measured.status, NeighborhoodComparisonStatus::Measured);
    assert_eq!(measured.retention, Some(1.0));
    assert_eq!(measured.compared_objects, 6);
    assert_eq!(measured.compared_neighbor_slots, 18);
    assert_eq!(measured.retained_neighbor_slots, 18);
    assert_eq!(measured.excluded_root_objects, 1);
    assert!(measure(&projection(&translated), &translated, Some(&old)).displacement > 0);

    let mut disrupted = old.clone();
    let left = disrupted.boxes["b"].bounds;
    let right = disrupted.boxes["e"].bounds;
    disrupted.boxes.get_mut("b").unwrap().bounds = right;
    disrupted.boxes.get_mut("e").unwrap().bounds = left;
    let measured = continuity(&disrupted, Some(&old));
    assert!(measured.retention.unwrap() < 1.0);
    assert_eq!(measured.compared_neighbor_slots, 18);
}

#[test]
fn insertion_and_deletion_compare_only_the_same_surviving_cohort() {
    let old = fixture();
    let mut inserted = old.clone();
    add_box(
        &mut inserted,
        "new",
        Some("root"),
        Point::new(5, 0, 0),
        true,
    );
    let measured = continuity(&inserted, Some(&old));
    assert_eq!(measured.added_objects, 1);
    assert_eq!(measured.retention, Some(1.0));
    assert_eq!(measured.compared_objects, 6);
    assert_eq!(measured.compared_neighbor_slots, 18);
    let mut removed = old.clone();
    removed.boxes.remove("b");
    let measured = continuity(&removed, Some(&old));
    assert_eq!(measured.removed_objects, 1);
    assert_eq!(measured.retention, Some(1.0));
    assert_eq!(measured.compared_objects, 5);
    assert_eq!(measured.compared_neighbor_slots, 15);
}

#[test]
fn relation_only_changes_do_not_change_neighborhoods_or_add_a_score_weight() {
    let old = fixture();
    let mut current = old.clone();
    current.revision += 1;
    current.projection_key = "new-relation-membership".into();
    current.routes.push(Route {
        edge_id: "request".into(),
        semantic_ids: vec!["request".into()],
        points: vec![Point::new(2, 0, 0), Point::new(8, 0, 0)],
        scope_ports: vec![],
    });
    let mut p = projection(&current);
    p.edges.push(Edge {
        id: "request".into(),
        semantic_ids: vec!["request".into()],
        source: "a".into(),
        target: "b".into(),
    });
    validate(&p, &current, &BTreeMap::new(), 2).unwrap();
    let with_previous = measure(&p, &current, Some(&old));
    let without_previous = measure(&p, &current, None);
    assert_eq!(with_previous.neighborhood.retention, Some(1.0));
    assert_eq!(
        with_previous.structural_cost,
        without_previous.structural_cost
    );
}

fn grouped_fixture() -> LayoutSolution {
    let mut result = fixture();
    add_box(
        &mut result,
        "group",
        Some("root"),
        Point::new(-250, 0, 0),
        false,
    );
    result.boxes.get_mut("group").unwrap().semantic_ids = vec!["g1".into(), "g2".into()];
    add_box(
        &mut result,
        "g1",
        Some("group"),
        Point::new(-280, 0, 0),
        true,
    );
    add_box(
        &mut result,
        "g2",
        Some("group"),
        Point::new(-220, 0, 0),
        true,
    );
    result
}

#[test]
fn reparenting_group_membership_and_representation_changes_have_explicit_exclusions() {
    let old = grouped_fixture();
    let mut moved = old.clone();
    moved.boxes.get_mut("b").unwrap().parent = Some("group".into());
    moved.boxes.get_mut("b").unwrap().bounds =
        old.boxes["b"].bounds.translated(Point::new(-260, 0, 0));
    let measured = continuity(&moved, Some(&old));
    assert_eq!(measured.excluded_reparented_objects, 1);
    assert_eq!(measured.retention, Some(1.0));

    let mut regrouped = old.clone();
    regrouped
        .boxes
        .get_mut("group")
        .unwrap()
        .semantic_ids
        .push("g3".into());
    let measured = continuity(&regrouped, Some(&old));
    assert_eq!(measured.excluded_representation_changes, 1);
    assert_eq!(measured.excluded_frame_changes, 2);
    assert_eq!(measured.compared_objects, 6);
    assert_eq!(measured.retention, Some(1.0));

    let old = fixture();
    let mut expanded = old.clone();
    expanded.boxes.get_mut("a").unwrap().solid = false;
    let measured = continuity(&expanded, Some(&old));
    assert_eq!(measured.excluded_representation_changes, 1);
    assert_eq!(measured.compared_objects, 5);
}

#[test]
fn changed_root_or_model_frames_are_not_compared() {
    let old = grouped_fixture();
    let mut entered = old.clone();
    entered
        .boxes
        .retain(|id, _| ["group", "g1", "g2"].contains(&id.as_str()));
    entered.boxes.get_mut("group").unwrap().parent = None;
    let measured = continuity(&entered, Some(&old));
    assert_eq!(
        measured.status,
        NeighborhoodComparisonStatus::IncompatibleFrame
    );
    assert_eq!(measured.retention, None);
    assert_eq!(measured.excluded_frame_changes, 3);
    let mut other = old.clone();
    other.model_id = "another-model".into();
    assert_eq!(
        continuity(&other, Some(&old)).status,
        NeighborhoodComparisonStatus::IncompatibleFrame
    );
}

#[test]
fn ties_semantic_member_order_and_projection_input_order_are_deterministic() {
    let mut old = grouped_fixture();
    for (id, center) in [
        ("a", Point::ZERO),
        ("b", Point::new(20, 0, 0)),
        ("c", Point::new(-20, 0, 0)),
        ("d", Point::new(0, 20, 0)),
        ("e", Point::new(0, -20, 0)),
        ("f", Point::new(0, 0, 20)),
    ] {
        add_box(&mut old, id, Some("root"), center, true);
    }
    let mut current = old.clone();
    current
        .boxes
        .get_mut("group")
        .unwrap()
        .semantic_ids
        .reverse();
    let expected = continuity(&current, Some(&old));
    assert_eq!(expected.retention, Some(1.0));
    let mut reordered = projection(&current);
    reordered.nodes.reverse();
    assert_eq!(
        measure(&reordered, &current, Some(&old)).neighborhood,
        expected
    );
}

#[test]
fn no_previous_empty_denominator_and_legacy_metrics_remain_unambiguous() {
    let old = fixture();
    assert_eq!(continuity(&old, None), NeighborhoodContinuity::default());
    let mut single = old.clone();
    single.boxes.retain(|id, _| id == "root" || id == "a");
    let measured = continuity(&single, Some(&single));
    assert_eq!(
        measured.status,
        NeighborhoodComparisonStatus::NoComparableNeighbors
    );
    assert_eq!(measured.retention, None);
    assert_eq!(measured.excluded_insufficient_peers, 1);
    assert_eq!(measured.compared_neighbor_slots, 0);
    let measured = measure(&projection(&old), &old, Some(&old));
    let mut legacy = serde_json::to_value(&measured).unwrap();
    legacy.as_object_mut().unwrap().remove("neighborhood");
    let loaded: Metrics = serde_json::from_value(legacy).unwrap();
    assert_eq!(loaded.neighborhood, NeighborhoodContinuity::default());
    assert_eq!(loaded.structural_cost, measured.structural_cost);
}

#[test]
fn comparison_work_stops_at_the_visible_representation_ceiling() {
    let mut many = fixture();
    for i in many.boxes.len()..1025 {
        add_box(
            &mut many,
            &format!("extra:{i}"),
            Some("root"),
            Point::new(i as i64 * 10, 0, 0),
            true,
        );
    }
    // Only the diagnostic is under test: no solver is asked to publish this
    // oversized scene, and measurement must not start a larger neighbor search.
    let measured = measure(&projection(&many), &many, Some(&many)).neighborhood;
    assert_eq!(measured.status, NeighborhoodComparisonStatus::LimitExceeded);
    assert_eq!(measured.retention, None);
    assert_eq!(measured.compared_neighbor_slots, 0);
}
