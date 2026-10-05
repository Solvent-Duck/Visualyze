use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use visualyze_layout::{
    Algorithm, Edge, LayoutError, Node, NodeKind, Point, Projection, QualityProfile,
    assembly::{AnchorPin, Assembly, DyadicTransform, ExactPoint},
    composed::{
        COMPOSITION_VERSION, CanonicalPins, ComposedMetrics, ComposedRequest, CompositionContext,
        persistence::{
            MAX_STATE_BYTES, PersistedComposedView, PersistenceError, STATE_VERSION, load, save,
        },
        solve_composed,
    },
    persistence::{DISMISSAL_ENCODING_VERSION, DismissedProposal, PersistedView},
    projection::{LayoutPolicy, ViewState},
};
use visualyze_model::provider::SemanticLens;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "visualyze-composed-persistence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("view.composed-v1.json")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn node(id: &str, parent: Option<&str>, solid: bool, band: i32) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![format!("semantic:{id}")],
        parent: parent.map(str::to_owned),
        kind: if solid {
            NodeKind::Service
        } else {
            NodeKind::Scope
        },
        solid,
        band,
        min_size: Point::new(12, 8, 12),
        preferred_size: Point::new(24, 16, 24),
        max_size: Point::new(48, 32, 48),
    }
}
fn fixture() -> (Projection, PersistedComposedView) {
    let p = Projection {
        key: "persistence-nested-v1".into(),
        model_id: "declared-persistence-fixture".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, false, 0),
            node("auth", Some("root"), true, 3),
            node("app", Some("root"), false, 0),
            node("inner", Some("app"), false, 0),
            node("leaf", Some("inner"), true, 0),
        ],
        edges: vec![Edge {
            id: "auth-leaf".into(),
            source: "auth".into(),
            target: "leaf".into(),
            semantic_ids: vec!["coverage:a".into(), "coverage:b".into()],
        }],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec![],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec![],
    };
    let accepted = solve_composed(ComposedRequest {
        projection: &p,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &CanonicalPins::new(),
        work_budget: 96,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    let leaf = accepted.assembly.frame("inner").unwrap().solution().boxes["leaf"]
        .bounds
        .center();
    let anchor_pins = vec![
        AnchorPin::capture(&accepted.assembly, "inner", "root", "leaf").unwrap(),
        AnchorPin::capture(&accepted.assembly, "inner", "inner", "leaf").unwrap(),
    ];
    let mut view = ViewState::default();
    view.frame_lens = Some(SemanticLens::Logical);
    view.enter("root");
    view.set_expanded("app", true);
    view.layer_preferences.insert("auth".into(), 3);
    let state = PersistedComposedView {
        version: STATE_VERSION,
        model_id: p.model_id.clone(),
        revision: p.revision,
        context: CompositionContext {
            model_id: p.model_id.clone(),
            root: p.root.clone(),
            lens: SemanticLens::Logical,
            policy: LayoutPolicy::Crud,
        },
        view,
        canonical_pins: BTreeMap::from([("inner".into(), BTreeMap::from([("leaf".into(), leaf)]))]),
        anchor_pins,
        constraint_sets: vec![],
        dismissed_proposals: vec![DismissedProposal {
            encoding_version: DISMISSAL_ENCODING_VERSION,
            layout_version: COMPOSITION_VERSION,
            model_id: p.model_id.clone(),
            root: p.root.clone(),
            lens: SemanticLens::Logical,
            signature: [17, 29],
        }],
        accepted,
    };
    (p, state)
}
fn write_json(path: &PathBuf, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

#[test]
fn exact_roundtrip_preserves_every_frame_fragment_pin_view_and_dismissal() {
    let (p, expected) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &expected, &cancel).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["accepted"].get("metrics").is_none());
    assert!(
        json["accepted"]["frames"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f.get("metrics").is_none())
    );
    let mut restored = load(&path, &cancel).unwrap();
    assert_eq!(restored.accepted.metrics, ComposedMetrics::default());
    assert_eq!(
        serde_json::to_value(&expected.view).unwrap(),
        serde_json::to_value(&restored.view).unwrap()
    );
    assert_eq!(restored.context, expected.context);
    assert_eq!(restored.canonical_pins, expected.canonical_pins);
    assert_eq!(restored.anchor_pins, expected.anchor_pins);
    assert_eq!(restored.dismissed_proposals, expected.dismissed_proposals);
    assert_eq!(restored.accepted.assembly, expected.accepted.assembly);
    assert_eq!(restored.accepted.routes, expected.accepted.routes);
    assert_eq!(restored.accepted.work, expected.accepted.work);
    restored.accepted.metrics.structural_cost = f64::NAN;
    restored.validate(&p, &cancel).unwrap();
    assert_eq!(restored.accepted.metrics, expected.accepted.metrics);
    assert_eq!(
        restored.accepted.present(&cancel).unwrap(),
        expected.accepted.present(&cancel).unwrap()
    );
    save(&path, &restored, &cancel).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn fractional_direct_translations_and_anchor_transform_origins_roundtrip_exactly() {
    let (p, mut state) = fixture();
    let s = &state.accepted.assembly;
    let frames = s
        .frame_ids()
        .map(|id| s.frame(id).unwrap().clone())
        .collect();
    let mut instances: Vec<_> = s
        .frame_ids()
        .map(|id| s.instance(id).unwrap().clone())
        .collect();
    let child = instances
        .iter_mut()
        .find(|instance| instance.frame_id == "inner")
        .unwrap();
    let original = child.transform;
    let shift = original.scale_shift();
    assert!(shift > 0);
    let old_translation = original.translation().to_integer().unwrap();
    let mut numerators =
        [old_translation.x, old_translation.y, old_translation.z].map(|n| i128::from(n) << shift);
    numerators[0] += 1;
    let origin = original.origin() + Point::new(1, 0, 0);
    child.transform =
        DyadicTransform::from_exact(origin, ExactPoint::new(numerators, shift).unwrap(), shift)
            .unwrap();
    assert_ne!(child.transform.translation().denominator_shift(), 0);
    state.accepted.assembly = Assembly::new(frames, instances).unwrap();
    state.anchor_pins =
        vec![AnchorPin::capture(&state.accepted.assembly, "inner", "root", "leaf").unwrap()];
    state.validate(&p, &AtomicBool::new(false)).unwrap();
    let temp = Temp::new();
    save(temp.path(), &state, &AtomicBool::new(false)).unwrap();
    let mut restored = load(temp.path(), &AtomicBool::new(false)).unwrap();
    restored.validate(&p, &AtomicBool::new(false)).unwrap();
    assert_eq!(restored.accepted.assembly, state.accepted.assembly);
    assert_eq!(restored.anchor_pins, state.anchor_pins);
}

#[test]
fn loading_shape_is_separate_from_certifying_current_model_geometry() {
    let (p, state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut changed = value.clone();
    let x = changed["accepted"]["routes"]["auth-leaf"]["fragments"][0]["points"][0]["x"]
        .as_i64()
        .unwrap();
    changed["accepted"]["routes"]["auth-leaf"]["fragments"][0]["points"][0]["x"] = (x + 1).into();
    write_json(&path, &changed);
    let mut invalid_geometry = load(&path, &cancel).unwrap();
    assert!(invalid_geometry.validate(&p, &cancel).is_err());
    assert_eq!(
        invalid_geometry.accepted.metrics,
        ComposedMetrics::default()
    );
    write_json(&path, &value);
    let mut restored = load(&path, &cancel).unwrap();
    for mismatch in 0..4 {
        let mut wrong = p.clone();
        match mismatch {
            0 => wrong.revision += 1,
            1 => wrong.model_id.push_str("-other"),
            2 => wrong.edges[0].target = "auth".into(),
            _ => wrong.key.push_str("-other"),
        }
        assert!(restored.validate(&wrong, &cancel).is_err());
    }
    restored.validate(&p, &cancel).unwrap();
}

#[test]
fn malformed_assembly_anchors_keys_membership_and_numeric_input_fail_without_modification() {
    let (_, state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for mutation in 0..13 {
        let mut bad = value.clone();
        let frames = bad["accepted"]["frames"].as_array_mut().unwrap();
        let child = frames.iter().position(|f| f["id"] == "inner").unwrap();
        match mutation {
            0 => frames[child]["parent"] = "missing".into(),
            1 => frames[child]["transform"]["shift"] = 255.into(),
            2 => {
                frames[child]["transform"]["translation"]["numerators"][0] =
                    "170141183460469231731687303715884105728".into()
            }
            3 => frames[child]["boxes"]["leaf"]["bounds"]["max"]["x"] = i64::MAX.into(),
            4 => frames[child]["model_id"] = "other-model".into(),
            5 => bad["anchor_pins"][0]["anchored"]["numerators"][0] = "999999".into(),
            6 => bad["anchor_pins"][0]["object_id"] = "absent".into(),
            7 => bad["canonical_pins"]["inner"]["leaf"]["x"] = i64::MAX.into(),
            8 => bad["accepted"]["work"]["evaluations"]["inner"] = u64::MAX.into(),
            9 => {
                bad["accepted"]["routes"]["auth-leaf"]["fragments"][0]["request_semantic_ids"] =
                    serde_json::json!(["different-claim"])
            }
            10 => bad["context"]["lens"] = "code".into(),
            11 => bad["accepted"]["metrics"] = serde_json::json!({"structural_cost":0}),
            _ => {
                bad["dismissed_proposals"][0]["layout_version"] =
                    visualyze_layout::LAYOUT_VERSION.into()
            }
        }
        write_json(&path, &bad);
        let before = std::fs::read(&path).unwrap();
        assert!(load(&path, &cancel).is_err(), "mutation {mutation}");
        assert!(
            save(&path, &state, &cancel).is_err(),
            "preserve mutation {mutation}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn version_one_is_read_without_writing_and_upgraded_only_after_validation() {
    let (p, state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["version"] = 1.into();
    old.as_object_mut().unwrap().remove("constraint_sets");
    write_json(&path, &old);
    let before = std::fs::read(&path).unwrap();
    let mut restored = load(&path, &cancel).unwrap();
    assert_eq!(restored.version, 1);
    assert!(restored.constraint_sets.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    restored.validate(&p, &cancel).unwrap();
    assert!(save(&path, &restored, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    restored.version = STATE_VERSION;
    save(&path, &restored, &cancel).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], STATE_VERSION);
    old["anchor_pins"][0]["anchored"]["numerators"][0] = "999999".into();
    write_json(&path, &old);
    let corrupt = std::fs::read(&path).unwrap();
    assert!(load(&path, &cancel).is_err());
    assert!(save(&path, &restored, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
}

#[test]
fn durable_registry_roundtrips_checked_links_and_rejects_active_conflicts_and_limits() {
    use visualyze_layout::composed::persistence::{
        PersistedConstraintSet, validate_constraint_registry,
    };
    let (p, mut state) = fixture();
    let cancel = AtomicBool::new(false);
    let current = PersistedConstraintSet {
        presentation: BTreeMap::new(),
        context: state.context.clone(),
        pins: state.canonical_pins.clone(),
        anchors: state.anchor_pins.clone(),
        ownership: BTreeMap::from([(
            "leaf".into(),
            vec!["leaf".into(), "inner".into(), "app".into(), "root".into()],
        )]),
    };
    state.constraint_sets.push(current.clone());
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancel).unwrap();
    let mut restored = load(&path, &cancel).unwrap();
    restored.validate(&p, &cancel).unwrap();
    assert_eq!(restored.constraint_sets, state.constraint_sets);
    let mut conflicting = restored.clone();
    conflicting.constraint_sets[0].anchors.clear();
    conflicting.constraint_sets[0]
        .pins
        .get_mut("inner")
        .unwrap()
        .get_mut("leaf")
        .unwrap()
        .x += 1;
    assert!(conflicting.validate(&p, &cancel).is_err());
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for mutation in 0..4 {
        let mut changed = value.clone();
        match mutation {
            0 => {
                changed["constraint_sets"][0]["anchors"][0]["links"][0]["parent"] = "missing".into()
            }
            1 => {
                changed["constraint_sets"][0]["ownership"]["leaf"] =
                    serde_json::json!(["other", "inner"])
            }
            2 => changed["constraint_sets"][0]["pins"]["inner"]["leaf"]["x"] = i64::MAX.into(),
            _ => {
                changed["constraint_sets"][0]["anchors"] = serde_json::json!([]);
                let x = changed["constraint_sets"][0]["pins"]["inner"]["leaf"]["x"]
                    .as_i64()
                    .unwrap();
                changed["constraint_sets"][0]["pins"]["inner"]["leaf"]["x"] = (x + 1).into();
            }
        }
        write_json(&path, &changed);
        let before = std::fs::read(&path).unwrap();
        assert!(load(&path, &cancel).is_err());
        assert!(save(&path, &state, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let mut too_many = current.clone();
    too_many.anchors.clear();
    too_many.ownership.clear();
    too_many.pins = BTreeMap::from([(
        "inner".into(),
        (0..=visualyze_layout::assembly::MAX_TOTAL_BOXES)
            .map(|i| (format!("pin:{i}"), Point::ZERO))
            .collect(),
    )]);
    assert!(validate_constraint_registry(&state.model_id, &[too_many], &cancel).is_err());
    let contexts: Vec<_> = (0..65)
        .map(|i| {
            let mut set = current.clone();
            set.context.root = format!("context:{i}");
            set
        })
        .collect();
    assert!(validate_constraint_registry(&state.model_id, &contexts, &cancel).is_err());
}

#[test]
fn version_three_child_owned_pins_require_bidirectional_repair_before_current_save() {
    use visualyze_layout::composed::{persistence::PersistedConstraintSet, resolve_constraints};
    let (p, mut current) = fixture();
    let cancelled = AtomicBool::new(false);
    let mut child_context = current.context.clone();
    child_context.root = "app".into();
    let child = PersistedConstraintSet {
        presentation: BTreeMap::new(),
        context: child_context,
        pins: current.canonical_pins.clone(),
        anchors: vec![
            AnchorPin::capture(&current.accepted.assembly, "inner", "app", "leaf").unwrap(),
        ],
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
        )]),
    };
    current.anchor_pins = child.anchors.clone();
    current.constraint_sets = vec![child.clone()];
    current.validate(&p, &cancelled).unwrap();
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &current, &cancelled).unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["version"] = 3.into();
    // A v3 ancestor view did not activate these child-owned constraints.
    old["canonical_pins"] = serde_json::json!({});
    old["anchor_pins"] = serde_json::json!([]);
    write_json(&path, &old);
    let original_bytes = std::fs::read(&path).unwrap();
    let mut restored = load(&path, &cancelled).unwrap();
    assert_eq!(restored.version, 3);
    restored.validate(&p, &cancelled).unwrap();
    assert_eq!(restored.constraint_sets, vec![child.clone()]);
    assert!(save(&path, &restored, &cancelled).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

    // Changing only the version cannot turn inactive old pins into validated
    // active constraints. The runtime must resolve and solve/revalidate them.
    restored.version = STATE_VERSION;
    assert!(restored.validate(&p, &cancelled).is_err());
    assert!(save(&path, &restored, &cancelled).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    let effective =
        resolve_constraints(&restored.context, &restored.constraint_sets, &cancelled).unwrap();
    restored.canonical_pins = effective.pins;
    restored.anchor_pins = effective.anchors;
    restored.validate(&p, &cancelled).unwrap();
    save(&path, &restored, &cancelled).unwrap();
    let mut reread = load(&path, &cancelled).unwrap();
    assert_eq!(reread.version, STATE_VERSION);
    assert_eq!(reread.canonical_pins, child.pins);
    assert_eq!(reread.anchor_pins, child.anchors);
    assert_eq!(
        reread.constraint_sets,
        vec![child],
        "the ancestor never owns a copied pin"
    );
    reread.validate(&p, &cancelled).unwrap();
}

#[test]
fn version_three_reading_preserves_downward_inheritance() {
    use visualyze_layout::composed::{persistence::PersistedConstraintSet, resolve_constraints};
    let (p, mut state) = fixture();
    let cancelled = AtomicBool::new(false);
    let mut ancestor_context = state.context.clone();
    ancestor_context.root = "outer".into();
    let ancestor = PersistedConstraintSet {
        presentation: BTreeMap::new(),
        context: ancestor_context,
        pins: state.canonical_pins.clone(),
        anchors: state.anchor_pins.clone(),
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "inner", "app", "root", "outer"]
                .map(String::from)
                .to_vec(),
        )]),
    };
    state.constraint_sets = vec![ancestor];
    let effective =
        resolve_constraints(&state.context, &state.constraint_sets, &cancelled).unwrap();
    state.anchor_pins = effective.anchors;
    state.validate(&p, &cancelled).unwrap();
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancelled).unwrap();
    let mut wire: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    wire["version"] = 3.into();
    write_json(&path, &wire);
    let mut loaded = load(&path, &cancelled).unwrap();
    assert_eq!(loaded.version, 3);
    assert_eq!(loaded.canonical_pins, state.canonical_pins);
    assert_eq!(loaded.anchor_pins, state.anchor_pins);
    loaded.validate(&p, &cancelled).unwrap();
    // Removing an active downward constraint remains corruption under v3.
    wire["canonical_pins"] = serde_json::json!({});
    wire["anchor_pins"] = serde_json::json!([]);
    write_json(&path, &wire);
    assert!(load(&path, &cancelled).is_err());
}

#[test]
fn version_four_preserves_bidirectional_pins_and_defaults_feedback_before_upgrade() {
    use visualyze_layout::composed::persistence::PersistedConstraintSet;
    let (p, mut state) = fixture();
    let cancel = AtomicBool::new(false);
    let mut child_context = state.context.clone();
    child_context.root = "app".into();
    let child = PersistedConstraintSet {
        presentation: BTreeMap::new(),
        context: child_context,
        pins: state.canonical_pins.clone(),
        anchors: vec![
            AnchorPin::capture(&state.accepted.assembly, "inner", "app", "leaf").unwrap(),
        ],
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
        )]),
    };
    state.anchor_pins = child.anchors.clone();
    state.constraint_sets = vec![child.clone()];
    state.validate(&p, &cancel).unwrap();
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancel).unwrap();
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["version"] = 4.into();
    let work = old["accepted"]["work"].as_object_mut().unwrap();
    work.remove("feedback_evaluations");
    work.remove("feedback_improvements");
    write_json(&path, &old);
    let original = std::fs::read(&path).unwrap();
    let mut restored = load(&path, &cancel).unwrap();
    assert_eq!(restored.version, 4);
    assert_eq!(restored.accepted.work.feedback_evaluations, 0);
    assert_eq!(restored.accepted.work.feedback_improvements, 0);
    assert_eq!(restored.canonical_pins, child.pins);
    assert_eq!(restored.anchor_pins, child.anchors);
    restored.validate(&p, &cancel).unwrap();
    assert!(save(&path, &restored, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // V4 already inherited pins upward; it must not fall back to v3 rules.
    let mut missing = old.clone();
    missing["canonical_pins"] = serde_json::json!({});
    missing["anchor_pins"] = serde_json::json!([]);
    write_json(&path, &missing);
    assert!(load(&path, &cancel).is_err());
    let corrupt = std::fs::read(&path).unwrap();
    restored.version = STATE_VERSION;
    assert!(save(&path, &restored, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);

    write_json(&path, &old);
    restored.validate(&p, &cancel).unwrap();
    save(&path, &restored, &cancel).unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(current["version"], STATE_VERSION);
    assert_eq!(current["accepted"]["work"]["feedback_evaluations"], 0);
    assert_eq!(current["accepted"]["work"]["feedback_improvements"], 0);
    assert_eq!(load(&path, &cancel).unwrap().constraint_sets, vec![child]);
}

#[test]
fn persisted_feedback_counts_share_budget_and_corruption_is_not_overwritten() {
    let (p, mut state) = fixture();
    let cancel = AtomicBool::new(false);
    let evaluations = state.accepted.work.evaluations.values().sum::<usize>();
    state.accepted.work.allocated = state.accepted.work.evaluations.clone();
    state.accepted.work.budget = evaluations + 3;
    state.accepted.work.feedback_evaluations = 3;
    state.accepted.work.feedback_improvements = 2;
    state.validate(&p, &cancel).unwrap();
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancel).unwrap();
    let mut loaded = load(&path, &cancel).unwrap();
    assert_eq!(loaded.accepted.work, state.accepted.work);
    loaded.validate(&p, &cancel).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for mutation in 0..4 {
        let mut changed = value.clone();
        let work = &mut changed["accepted"]["work"];
        match mutation {
            0 => work["feedback_evaluations"] = 4.into(),
            1 => work["feedback_improvements"] = 4.into(),
            2 => {
                work["budget"] = serde_json::json!(usize::MAX);
                work["feedback_evaluations"] = serde_json::json!(usize::MAX);
            }
            _ => work["feedback_evaluations"] = (-1).into(),
        }
        write_json(&path, &changed);
        let corrupt = std::fs::read(&path).unwrap();
        assert!(load(&path, &cancel).is_err(), "mutation {mutation}");
        assert!(save(&path, &state, &cancel).is_err(), "mutation {mutation}");
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}

#[test]
fn unknown_versions_foreign_newer_files_and_flat_saves_are_never_migrated() {
    let (_, state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for mutation in 0..3 {
        let mut protected = value.clone();
        if mutation == 0 {
            protected["version"] = 99.into();
        }
        if mutation == 1 {
            protected["model_id"] = "other-model".into();
            protected["context"]["model_id"] = "other-model".into();
            protected["accepted"]["model_id"] = "other-model".into();
            protected["dismissed_proposals"] = serde_json::json!([]);
            for f in protected["accepted"]["frames"].as_array_mut().unwrap() {
                f["model_id"] = "other-model".into();
            }
        }
        if mutation == 2 {
            protected["revision"] = 2.into();
            protected["accepted"]["revision"] = 2.into();
            for f in protected["accepted"]["frames"].as_array_mut().unwrap() {
                f["revision"] = 2.into();
            }
        }
        write_json(&path, &protected);
        let before = std::fs::read(&path).unwrap();
        if mutation == 0 {
            assert!(matches!(
                load(&path, &cancel),
                Err(PersistenceError::UnsupportedVersion(99))
            ));
        } else {
            assert!(load(&path, &cancel).is_ok());
        }
        assert!(save(&path, &state, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let flat_path = temp.0.join("view.flat-v1.json");
    let flat = PersistedView {
        version: visualyze_layout::persistence::STATE_VERSION,
        model_id: state.model_id.clone(),
        revision: state.revision,
        view: state.view.clone(),
        accepted: state
            .accepted
            .assembly
            .frame("root")
            .unwrap()
            .solution()
            .clone(),
        dismissed_proposals: vec![],
    };
    visualyze_layout::persistence::save(&flat_path, &flat).unwrap();
    let flat_before = std::fs::read(&flat_path).unwrap();
    assert!(load(&flat_path, &cancel).is_err());
    assert!(save(&flat_path, &state, &cancel).is_err());
    assert_eq!(std::fs::read(&flat_path).unwrap(), flat_before);
    assert_eq!(
        visualyze_layout::persistence::load(&flat_path)
            .unwrap()
            .accepted,
        flat.accepted
    );
}

#[test]
fn duplicate_keys_byte_and_depth_limits_cancelled_operations_and_retry() {
    let (p, mut state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let valid = std::fs::read(&path).unwrap();
    let mut duplicate = b"{\"version\":1,".to_vec();
    duplicate.extend_from_slice(&valid[1..]);
    for bad in [
        duplicate,
        vec![b' '; MAX_STATE_BYTES + 1],
        format!("{}0{}", "[".repeat(25), "]".repeat(25)).into_bytes(),
    ] {
        std::fs::write(&path, &bad).unwrap();
        assert!(load(&path, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
    }
    std::fs::write(&path, &valid).unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        load(&path, &cancel),
        Err(PersistenceError::Layout(LayoutError::Cancelled))
    ));
    assert!(matches!(
        save(&path, &state, &cancel),
        Err(PersistenceError::Layout(LayoutError::Cancelled))
    ));
    assert!(matches!(
        state.validate(&p, &cancel),
        Err(PersistenceError::Layout(LayoutError::Cancelled))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), valid);
    cancel.store(false, Ordering::Relaxed);
    let mut restored = load(&path, &cancel).unwrap();
    restored.validate(&p, &cancel).unwrap();
    save(&path, &restored, &cancel).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), valid);
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn links_and_directories_are_rejected_and_existing_targets_preserved() {
    let (_, state) = fixture();
    let temp = Temp::new();
    let path = temp.path();
    let cancel = AtomicBool::new(false);
    save(&path, &state, &cancel).unwrap();
    let before = std::fs::read(&path).unwrap();
    let link = temp.0.join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    for target in [&link, &temp.0] {
        assert!(load(target, &cancel).is_err());
        assert!(save(target, &state, &cancel).is_err());
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn protected_presentation_fixture() -> (Projection, PersistedComposedView) {
    use visualyze_layout::{
        composed::persistence::{PersistedConstraintSet, PinPresentation},
        projection::DisplayGroup,
    };
    let (mut projection, mut state) = fixture();
    projection.key = "protected-visual-group".into();
    let group = DisplayGroup {
        id: "inner".into(),
        parent: "app".into(),
        members: vec!["leaf".into(), "other".into()],
        suggested: true,
    };
    let inner = projection
        .nodes
        .iter_mut()
        .find(|n| n.id == "inner")
        .unwrap();
    inner.kind = NodeKind::Group;
    inner.semantic_ids = group.members.clone();
    projection
        .nodes
        .iter_mut()
        .find(|n| n.id == "leaf")
        .unwrap()
        .semantic_ids = vec!["leaf".into()];
    let mut other = node("other", Some("inner"), true, 0);
    other.semantic_ids = vec!["other".into()];
    projection.nodes.push(other);
    projection.suggested_groups = vec![group.clone()];
    state.accepted = solve_composed(ComposedRequest {
        projection: &projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::default(),
        pins: &CanonicalPins::new(),
        work_budget: 128,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    let anchor = AnchorPin::capture(&state.accepted.assembly, "inner", "app", "leaf").unwrap();
    state.canonical_pins = BTreeMap::from([(
        "inner".into(),
        BTreeMap::from([("leaf".into(), anchor.local_point())]),
    )]);
    state.anchor_pins = vec![anchor];
    let mut child_context = state.context.clone();
    child_context.root = "app".into();
    state.constraint_sets = vec![PersistedConstraintSet {
        context: child_context,
        pins: state.canonical_pins.clone(),
        anchors: state.anchor_pins.clone(),
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "app", "root"].map(String::from).to_vec(),
        )]),
        presentation: BTreeMap::from([(
            "leaf".into(),
            PinPresentation {
                path: ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
                groups: vec![group],
                reference: None,
                supplied: BTreeMap::from([
                    (
                        "leaf".into(),
                        ["leaf", "app", "root"].map(String::from).to_vec(),
                    ),
                    (
                        "other".into(),
                        ["other", "app", "root"].map(String::from).to_vec(),
                    ),
                    ("app".into(), ["app", "root"].map(String::from).to_vec()),
                ]),
            },
        )]),
    }];
    (projection, state)
}

#[test]
fn version_six_roundtrips_protected_groups_independently_of_suggestion_retention() {
    let (projection, state) = protected_presentation_fixture();
    let cancelled = AtomicBool::new(false);
    assert!(state.view.retained_suggestions().is_empty());
    assert!(state.view.group_overrides.is_empty());
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancelled).unwrap();
    let mut loaded = load(&path, &cancelled).unwrap();
    loaded.validate(&projection, &cancelled).unwrap();
    assert_eq!(loaded.constraint_sets, state.constraint_sets);
    assert_eq!(loaded.canonical_pins, state.canonical_pins);
    assert_eq!(loaded.anchor_pins, state.anchor_pins);
    assert_eq!(loaded.constraint_sets[0].context.root, "app");
    assert_eq!(
        loaded.context.root, "root",
        "effective pins have not become owned root pins"
    );
    let wire: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(wire["version"], 6);
    assert_eq!(
        wire["constraint_sets"][0]["presentation"]["leaf"]["groups"][0]["id"],
        "inner"
    );
}

#[test]
fn old_versions_cannot_smuggle_presentation_metadata_or_skip_group_pin_repair() {
    use visualyze_layout::composed::resolve_constraints;
    let (projection, state) = protected_presentation_fixture();
    let cancelled = AtomicBool::new(false);
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancelled).unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for version in 1..=5 {
        for presentation in [
            current["constraint_sets"][0]["presentation"].clone(),
            serde_json::json!({}),
        ] {
            let mut smuggled = current.clone();
            smuggled["version"] = version.into();
            smuggled["constraint_sets"][0]["presentation"] = presentation;
            write_json(&path, &smuggled);
            let before = std::fs::read(&path).unwrap();
            assert!(load(&path, &cancelled).is_err(), "version {version}");
            assert!(save(&path, &state, &cancelled).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
    // V5 lacked the visual frame in the supplied path, so this child-owned
    // pin was dormant in its ancestor even though its member was supplied.
    let mut old = current;
    old["version"] = 5.into();
    old["constraint_sets"][0]
        .as_object_mut()
        .unwrap()
        .remove("presentation");
    old["canonical_pins"] = serde_json::json!({});
    old["anchor_pins"] = serde_json::json!([]);
    write_json(&path, &old);
    let before = std::fs::read(&path).unwrap();
    let mut loaded = load(&path, &cancelled).unwrap();
    loaded.validate(&projection, &cancelled).unwrap();
    assert!(loaded.constraint_sets[0].presentation.is_empty());
    assert!(loaded.canonical_pins.is_empty());
    assert!(save(&path, &loaded, &cancelled).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // Runtime supplies and rechecks the captured presentation before repair.
    // Changing the version and adding evidence is not itself acceptance.
    loaded.version = STATE_VERSION;
    loaded.constraint_sets[0].presentation = state.constraint_sets[0].presentation.clone();
    assert!(loaded.validate(&projection, &cancelled).is_err());
    assert!(save(&path, &loaded, &cancelled).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let effective =
        resolve_constraints(&loaded.context, &loaded.constraint_sets, &cancelled).unwrap();
    loaded.canonical_pins = effective.pins;
    loaded.anchor_pins = effective.anchors;
    loaded.validate(&projection, &cancelled).unwrap();
    save(&path, &loaded, &cancelled).unwrap();
    let mut repaired = load(&path, &cancelled).unwrap();
    repaired.validate(&projection, &cancelled).unwrap();
    assert_eq!(repaired.version, 6);
    assert_eq!(repaired.constraint_sets, state.constraint_sets);
}

#[test]
fn corrupt_protected_evidence_is_not_loaded_or_overwritten() {
    let (_, state) = protected_presentation_fixture();
    let cancelled = AtomicBool::new(false);
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancelled).unwrap();
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for mutation in 0..4 {
        let mut wire = original.clone();
        let metadata = &mut wire["constraint_sets"][0]["presentation"]["leaf"];
        match mutation {
            0 => metadata["path"] = serde_json::json!(["leaf", "app", "root"]),
            1 => metadata["supplied"]["other"] = serde_json::json!(["other", "wrong"]),
            2 => metadata["reference"] = serde_json::json!({"origin":"inner", "target":"other"}),
            _ => metadata["unrecognized"] = true.into(),
        }
        write_json(&path, &wire);
        let before = std::fs::read(&path).unwrap();
        assert!(load(&path, &cancelled).is_err(), "mutation {mutation}");
        assert!(save(&path, &state, &cancelled).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn dormant_reference_roundtrips_without_applying_to_an_ancestors_target_or_reference() {
    use visualyze_layout::composed::persistence::{
        PersistedConstraintSet, PinPresentation, PinReference,
    };
    let (projection, mut state) = fixture();
    let cancelled = AtomicBool::new(false);
    state.constraint_sets.push(PersistedConstraintSet {
        context: state.context.clone(),
        pins: state.canonical_pins.clone(),
        anchors: state.anchor_pins.clone(),
        ownership: BTreeMap::from([(
            "leaf".into(),
            ["leaf", "inner", "app", "root"].map(String::from).to_vec(),
        )]),
        presentation: BTreeMap::new(),
    });
    let mut origin = state.context.clone();
    origin.root = "app".into();
    state.constraint_sets.push(PersistedConstraintSet {
        context: origin,
        pins: BTreeMap::from([(
            "app".into(),
            BTreeMap::from([("reference".into(), Point::new(1, 2, 3))]),
        )]),
        anchors: vec![],
        ownership: BTreeMap::new(),
        presentation: BTreeMap::from([(
            "reference".into(),
            PinPresentation {
                path: ["reference", "app", "root"].map(String::from).to_vec(),
                groups: vec![],
                reference: Some(PinReference {
                    origin: "app".into(),
                    target: "auth".into(),
                }),
                supplied: BTreeMap::from([
                    ("app".into(), ["app", "root"].map(String::from).to_vec()),
                    ("auth".into(), ["auth", "root"].map(String::from).to_vec()),
                ]),
            },
        )]),
    });
    let temp = Temp::new();
    let path = temp.path();
    save(&path, &state, &cancelled).unwrap();
    let mut loaded = load(&path, &cancelled).unwrap();
    loaded.validate(&projection, &cancelled).unwrap();
    assert_eq!(loaded.constraint_sets, state.constraint_sets);
    assert_eq!(loaded.canonical_pins, state.canonical_pins);
    assert_eq!(loaded.anchor_pins, state.anchor_pins);
    let metadata = &loaded.constraint_sets[1].presentation["reference"];
    assert_eq!(metadata.reference.as_ref().unwrap().origin, "app");
    assert_eq!(metadata.reference.as_ref().unwrap().target, "auth");
}
