use std::{collections::BTreeSet, path::PathBuf, sync::atomic::AtomicBool};
use visualyze_layout::{
    detail::{PlanBudget, plan_provider, plan_snapshot},
    noise::{NOISE_THRESHOLD, region_noise, visual_noise},
    projection::{ViewState, project},
    provider_projection::{ProviderViewRequest, project_provider},
};
use visualyze_model::{architecture::Architecture, provider::*};

/// `scopes` lists the service count of each scope under the root. Every scope
/// and service shares the default layer, so all of them compete for one row.
fn system(scopes: &[usize], root_services: usize) -> Architecture {
    let mut entities = vec![serde_json::json!({
        "id": "scope:root", "kind": "scope", "label": "Root", "boundary": "internal",
        "subtype": "system", "basis": "declared"
    })];
    let mut relations = vec![];
    let mut contain = |source: &str, target: &str| {
        relations.push(serde_json::json!({
            "id": format!("rel:contains:{target}"), "source": source, "target": target,
            "kind": "contains", "basis": "declared"
        }));
    };
    for (index, services) in scopes.iter().enumerate() {
        let scope = format!("scope:s{index}");
        entities.push(serde_json::json!({
            "id": scope, "kind": "scope", "label": scope, "boundary": "internal", "basis": "declared"
        }));
        contain("scope:root", &scope);
        for service in 0..*services {
            let id = format!("service:s{index}-{service:02}");
            entities.push(serde_json::json!({
                "id": id, "kind": "service", "label": id, "boundary": "internal", "basis": "declared"
            }));
            contain(&scope, &id);
        }
    }
    for service in 0..root_services {
        let id = format!("service:root-{service:02}");
        entities.push(serde_json::json!({
            "id": id, "kind": "service", "label": id, "boundary": "internal", "basis": "declared"
        }));
        contain("scope:root", &id);
    }
    let value = serde_json::json!({
        "schema_version": "0.1.0-draft.1",
        "model_id": "detail-planning",
        "revision": 1,
        "system_id": "scope:root",
        "snapshot": {
            "id": "detail-planning-001", "source_revision": null, "coverage": "partial",
            "scope_description": "Detail planning fixture", "omissions": []
        },
        "entities": entities,
        "relations": relations,
        "evidence": []
    });
    Architecture::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn logical() -> ViewState {
    let mut view = ViewState::default();
    view.lens = Some(SemanticLens::Logical);
    view
}

fn open(id: &str, planned: &visualyze_layout::Projection) -> bool {
    planned
        .nodes
        .iter()
        .any(|node| node.parent.as_deref() == Some(id))
}

#[test]
fn three_small_scopes_all_open_within_the_threshold() {
    let model = system(&[3, 4, 3], 0);
    let view = logical();
    let base = project(&model, &view).unwrap();
    assert_eq!(base.nodes.len(), 4);
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:s0", "scope:s1", "scope:s2"]
    );
    assert_eq!(planned.result.nodes.len(), 1 + 3 + 10);
    assert!(visual_noise(&planned.result).total <= NOISE_THRESHOLD);
    // The supplied services are shown, not suggested groups.
    assert!(planned.result.suggested_groups.is_empty());
    assert!(view.planned_ids().is_empty());
}

/// Suggested groups the projection actually draws (it also lists the ones it
/// could make for a collapsed box).
fn drawn_groups(projection: &visualyze_layout::Projection) -> Vec<&str> {
    projection
        .suggested_groups
        .iter()
        .filter(|group| projection.nodes.iter().any(|node| node.id == group.id))
        .map(|group| group.id.as_str())
        .collect()
}

fn opened_ids(
    planned: &visualyze_layout::detail::Planned<visualyze_layout::Projection>,
) -> Vec<&str> {
    planned.opened.iter().map(String::as_str).collect()
}

#[test]
fn each_scope_opens_by_its_own_interior_whatever_the_others_hold() {
    // Interiors of 10, 2, 3, 12 and 30 services. Twelve fit one row; thirty do
    // not, and no suggested group or page is invented to make them fit.
    let model = system(&[10, 2, 3, 12, 30], 0);
    let view = logical();
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(
        opened_ids(&planned),
        ["scope:s0", "scope:s1", "scope:s2", "scope:s3"]
    );
    assert!(!open("scope:s4", &planned.result));
    // Each opened interior is its own region within the threshold, and the view
    // holds only the five scopes.
    let regions = region_noise(&planned.result);
    for scope in ["scope:s0", "scope:s1", "scope:s2", "scope:s3"] {
        assert!(regions[scope].total <= NOISE_THRESHOLD, "{scope}");
    }
    assert_eq!(visual_noise(&planned.result).boxes, 5.0);
    assert!(visual_noise(&planned.result).total <= NOISE_THRESHOLD);
    // The projection is exactly the one the user would get by opening those.
    let mut by_hand = view.clone();
    for id in &planned.opened {
        by_hand.set_expanded(id, true);
    }
    assert_eq!(project(&model, &by_hand).unwrap().key, planned.result.key);
    // The same verdict whichever scopes are beside it, alone or together.
    for sizes in [&[30][..], &[30, 2], &[2, 30, 12], &[12, 12, 12, 12, 12, 30]] {
        let model = system(sizes, 0);
        let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
        for (index, size) in sizes.iter().enumerate() {
            assert_eq!(
                planned.opened.contains(&format!("scope:s{index}")),
                *size <= 12,
                "{sizes:?} scope {index}"
            );
        }
        assert!(drawn_groups(&planned.result).is_empty());
    }
}

#[test]
fn a_dense_focused_view_still_opens_the_boxes_whose_interiors_are_quiet() {
    // Twenty root services beside two scopes: the root itself is over the
    // threshold, which says nothing about what the scopes hold.
    let model = system(&[2, 2], 20);
    let view = logical();
    let base = project(&model, &view).unwrap();
    assert!(visual_noise(&base).total > NOISE_THRESHOLD);
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(opened_ids(&planned), ["scope:s0", "scope:s1"]);
    assert!(open("scope:s0", &planned.result));
    // Opening them did not make the root any denser: each counts as one box.
    assert_eq!(
        visual_noise(&planned.result).total,
        visual_noise(&base).total
    );
}

#[test]
fn an_explicit_collapse_is_never_reopened() {
    let model = system(&[3, 4, 3], 0);
    let mut view = logical();
    view.set_expanded("scope:s1", false);
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:s0", "scope:s2"]
    );
    assert!(!open("scope:s1", &planned.result));
    let s1 = planned
        .result
        .nodes
        .iter()
        .find(|n| n.id == "scope:s1")
        .unwrap();
    assert!(s1.solid);
}

#[test]
fn explicit_expansion_is_preserved_and_never_claimed_by_the_planner() {
    let model = system(&[3, 4, 3], 0);
    let mut view = logical();
    view.set_expanded("scope:s2", true);
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert!(open("scope:s2", &planned.result));
    assert!(!planned.opened.contains("scope:s2"));
    assert_eq!(planned.opened.len(), 2);
    assert!(view.expanded.contains("scope:s2"));
    assert!(view.planned_ids().is_empty());
}

#[test]
fn interface_pads_are_not_structure_worth_opening() {
    // Services expose interfaces, which are pads on their own face.
    let model = Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
        .unwrap();
    let view = logical();
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert!(planned.opened.is_empty());
    assert_eq!(planned.result.key, project(&model, &view).unwrap().key);
}

#[test]
fn nested_scopes_open_as_their_parents_reveal_them() {
    // root > outer > (inner > 3 services, 2 services)
    let json = serde_json::json!({
        "schema_version": "0.1.0-draft.1", "model_id": "nested", "revision": 1,
        "system_id": "scope:root",
        "snapshot": {"id": "n-1", "source_revision": null, "coverage": "partial",
            "scope_description": "nested", "omissions": []},
        "entities": [
            {"id": "scope:root", "kind": "scope", "label": "Root", "boundary": "internal", "subtype": "system", "basis": "declared"},
            {"id": "scope:outer", "kind": "scope", "label": "Outer", "boundary": "internal", "basis": "declared"},
            {"id": "scope:inner", "kind": "scope", "label": "Inner", "boundary": "internal", "basis": "declared"},
            {"id": "service:a", "kind": "service", "label": "A", "boundary": "internal", "basis": "declared"},
            {"id": "service:b", "kind": "service", "label": "B", "boundary": "internal", "basis": "declared"},
            {"id": "service:c", "kind": "service", "label": "C", "boundary": "internal", "basis": "declared"},
            {"id": "service:d", "kind": "service", "label": "D", "boundary": "internal", "basis": "declared"}
        ],
        "relations": [
            {"id": "r1", "source": "scope:root", "target": "scope:outer", "kind": "contains", "basis": "declared"},
            {"id": "r2", "source": "scope:outer", "target": "scope:inner", "kind": "contains", "basis": "declared"},
            {"id": "r3", "source": "scope:inner", "target": "service:a", "kind": "contains", "basis": "declared"},
            {"id": "r4", "source": "scope:inner", "target": "service:b", "kind": "contains", "basis": "declared"},
            {"id": "r5", "source": "scope:inner", "target": "service:c", "kind": "contains", "basis": "declared"},
            {"id": "r6", "source": "scope:outer", "target": "service:d", "kind": "contains", "basis": "declared"}
        ],
        "evidence": []
    });
    let model = Architecture::from_json(&serde_json::to_vec(&json).unwrap()).unwrap();
    // One level opens automatically: the focused root's direct children. The
    // scope that outer reveals stays closed until the user opens it.
    let planned = plan_snapshot(&model, &logical(), PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(opened_ids(&planned), ["scope:outer"]);
    assert!(planned.result.nodes.iter().any(|n| n.id == "scope:inner"));
    assert!(!planned.result.nodes.iter().any(|n| n.id == "service:a"));
    // Entering outer makes it the focus, which is planned with its own level.
    let mut inside = logical();
    inside.focus = Some("scope:outer".into());
    let planned = plan_snapshot(&model, &inside, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(opened_ids(&planned), ["scope:inner"]);
    assert!(planned.result.nodes.iter().any(|n| n.id == "service:a"));
}

#[test]
fn the_trial_limit_bounds_the_work() {
    // 3 and 30 services: the first batch opens both, 30 does not fit, and the
    // one that does is projected again alone, which is a second trial.
    let model = system(&[3, 30], 0);
    let view = logical();
    for trials in [0, 1] {
        let planned = plan_snapshot(&model, &view, PlanBudget { max_trials: trials }).unwrap();
        assert!(planned.opened.is_empty(), "{trials}");
        assert_eq!(planned.result.key, project(&model, &view).unwrap().key);
    }
    let planned = plan_snapshot(&model, &view, PlanBudget { max_trials: 2 }).unwrap();
    assert_eq!(opened_ids(&planned), ["scope:s0"]);
    // When everything fits, one trial is enough.
    let model = system(&[3, 4], 0);
    let planned = plan_snapshot(&model, &view, PlanBudget { max_trials: 1 }).unwrap();
    assert_eq!(opened_ids(&planned), ["scope:s0", "scope:s1"]);
}

/// What the planner opens, as ids.
fn planned_set(model: &Architecture, view: &ViewState) -> BTreeSet<String> {
    plan_snapshot(model, view, PlanBudget::SNAPSHOT)
        .unwrap()
        .opened
}

#[test]
fn a_sibling_never_decides_whether_a_box_opens_snapshot() {
    // Quiet scopes beside an 18-service one (a row of eighteen is noisy), a
    // 30-service one, and a dense root (eight services and six scopes in one
    // row). Expanding the noisy scopes explicitly would put their boxes into
    // the same budget as the others under one global rule; here each is a
    // region of its own.
    let model = system(&[3, 4, 12, 18, 30, 2], 8);
    let view = logical();
    let baseline = planned_set(&model, &view);
    assert_eq!(
        baseline.iter().map(String::as_str).collect::<Vec<_>>(),
        ["scope:s0", "scope:s1", "scope:s2", "scope:s5"]
    );
    for index in 0..6 {
        let sibling = format!("scope:s{index}");
        let others: BTreeSet<String> = baseline
            .iter()
            .filter(|id| **id != sibling)
            .cloned()
            .collect();
        // Expanding the sibling explicitly...
        let mut expanded = view.clone();
        expanded.set_expanded(&sibling, true);
        let planned = plan_snapshot(&model, &expanded, PlanBudget::SNAPSHOT).unwrap();
        assert_eq!(planned.opened, others, "expanding {sibling}");
        assert!(open(&sibling, &planned.result));
        // ...and collapsing it explicitly.
        let mut collapsed = view.clone();
        collapsed.set_expanded(&sibling, false);
        assert_eq!(
            planned_set(&model, &collapsed),
            others,
            "collapsing {sibling}"
        );
    }
    // The dense scope open and a quiet one collapsed, together.
    let mut both = view.clone();
    both.set_expanded("scope:s3", true);
    both.set_expanded("scope:s0", false);
    assert_eq!(
        planned_set(&model, &both)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:s1", "scope:s2", "scope:s5"]
    );
    // The expanded noisy scope is reported dense in its own region only.
    let planned = plan_snapshot(&model, &both, PlanBudget::SNAPSHOT).unwrap();
    let regions = region_noise(&planned.result);
    assert!(regions["scope:s3"].over_threshold());
    assert!(!visual_noise(&planned.result).over_threshold());
}

#[test]
fn planning_is_deterministic_and_does_not_touch_the_users_view() {
    let model = system(&[10, 2, 3, 12, 4], 0);
    let mut view = logical();
    view.set_expanded("scope:s0", false);
    let first = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    for _ in 0..3 {
        let again = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
        assert_eq!(again.opened, first.opened);
        assert_eq!(again.result.key, first.result.key);
    }
    assert!(view.planned_ids().is_empty());
    // The planner's clone never reaches a saved view.
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("planned"));
}

fn bundle() -> LocalBundle {
    LocalBundle::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl"),
        Limits::default(),
    )
    .unwrap()
}

fn request(view: &ViewState) -> ProviderViewRequest {
    ProviderViewRequest {
        focus: view.focus.clone(),
        lens: Some(SemanticLens::Logical),
        expanded: view.expanded_ids(),
        policy: view.policy,
        ..ProviderViewRequest::default()
    }
}

#[test]
fn bundle_planning_opens_supplied_containers_and_keeps_interface_pads_closed() {
    let cancel = AtomicBool::new(false);
    let mut provider = bundle();
    let view = ViewState::default();
    let base = project_provider(&mut provider, &request(&view), &cancel).unwrap();
    // The supplied datastore holds tables, and a table holds a column; only the
    // datastore hides supplied structure at the root.
    assert!(
        base.graph
            .nodes
            .iter()
            .all(|node| node.solid || node.id == base.graph.root)
    );
    let planned =
        plan_provider(&mut provider, &view, request, PlanBudget::BUNDLE, &cancel).unwrap();
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["datastore:orders"]
    );
    let graph = &planned.result.graph;
    let parent_of = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| node.parent.clone())
    };
    assert_eq!(
        parent_of("table:orders").as_deref(),
        Some("datastore:orders")
    );
    // One level opens: the table that the datastore reveals stays closed until
    // the user opens it or enters it.
    assert!(
        graph
            .nodes
            .iter()
            .all(|node| node.id != "column:customer-id")
    );
    assert!(
        graph
            .nodes
            .iter()
            .find(|node| node.id == "table:orders")
            .unwrap()
            .solid
    );
    assert!(visual_noise(graph).total <= NOISE_THRESHOLD);
    // Components without supplied structure stay boxes.
    for id in ["service:orders", "service:worker", "client:web"] {
        assert!(graph.nodes.iter().find(|node| node.id == id).unwrap().solid);
    }
    // The same request with the opened boxes made explicit gives the same view.
    let mut by_hand = view.clone();
    for id in &planned.opened {
        by_hand.set_expanded(id, true);
    }
    let explicit = project_provider(&mut provider, &request(&by_hand), &cancel).unwrap();
    assert_eq!(explicit.graph.key, graph.key);
    // An explicit collapse wins, and the result is deterministic.
    let mut collapsed = view.clone();
    collapsed.set_expanded("datastore:orders", false);
    let kept = plan_provider(
        &mut provider,
        &collapsed,
        request,
        PlanBudget::BUNDLE,
        &cancel,
    )
    .unwrap();
    assert!(kept.opened.is_empty());
    let again = plan_provider(&mut provider, &view, request, PlanBudget::BUNDLE, &cancel).unwrap();
    assert_eq!(again.opened, planned.opened);
    assert_eq!(again.result.graph.key, graph.key);
    assert!(view.planned_ids().is_empty());
}

/// A temporary bundle: scopes of 3, 6 and 30 services under the root.
fn sized_bundle(name: &str) -> (LocalBundle, PathBuf) {
    scopes_bundle(name, &[("small", 3), ("mid", 6), ("wide", 30)])
}

/// A temporary bundle of named scopes with the given service counts.
fn scopes_bundle(name: &str, scopes: &[(&str, usize)]) -> (LocalBundle, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "visualyze-detail-{}-{name}.jsonl",
        std::process::id()
    ));
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: "detail-sizes".into(),
            revision: 1,
            root_id: "root".into(),
            profiles: vec![Profile::Architecture],
            coverage: Coverage::Complete,
            scope_description: "Containers of different sizes".into(),
            omissions: vec![],
        },
        Limits::default(),
    )
    .unwrap();
    let mut entities = vec![("root".to_owned(), EntityKind::Scope)];
    let mut contains = vec![];
    for (scope, count) in scopes.iter().copied() {
        let scope = format!("scope:{scope}");
        entities.push((scope.clone(), EntityKind::Scope));
        contains.push(("root".to_owned(), scope.clone()));
        for i in 0..count {
            let service = format!("{scope}:service:{i:02}");
            entities.push((service.clone(), EntityKind::Service));
            contains.push((scope.clone(), service));
        }
    }
    entities.sort_by(|a, b| a.0.cmp(&b.0));
    contains.sort_by(|a, b| a.1.cmp(&b.1));
    for (id, kind) in &entities {
        writer
            .push(&BundleRecord::Entity(EntityRecord {
                summary: EntitySummary {
                    id: id.clone(),
                    kind: *kind,
                    label: id.clone(),
                    boundary: Boundary::Internal,
                    claim: Claim::declared(),
                },
                detail: None,
            }))
            .unwrap();
    }
    for (source, target) in contains {
        writer
            .push(&BundleRecord::Relation(RelationRecord {
                id: format!("contains:{target}"),
                kind: RelationKind::Contains,
                source,
                target,
                claim: Claim::declared(),
                label: None,
                aspect: None,
            }))
            .unwrap();
    }
    (writer.finish().unwrap(), path)
}

#[test]
fn bundle_planning_keeps_a_container_closed_when_its_interior_is_too_noisy() {
    let (mut provider, path) = sized_bundle("noisy");
    let cancel = AtomicBool::new(false);
    let view = ViewState::default();
    let planned =
        plan_provider(&mut provider, &view, request, PlanBudget::BUNDLE, &cancel).unwrap();
    let _ = std::fs::remove_file(&path);
    // Smallest first; thirty services do not fit beside them in one row, and no
    // suggested group or page is invented to make them fit.
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:mid", "scope:small"]
    );
    let graph = &planned.result.graph;
    assert!(visual_noise(graph).total <= NOISE_THRESHOLD);
    assert!(graph.suggested_groups.is_empty());
    assert!(
        graph
            .nodes
            .iter()
            .find(|n| n.id == "scope:wide")
            .unwrap()
            .solid
    );
    // The trial limit bounds the work: the first batch holds all three, the wide
    // one does not fit, and the two that do are projected again.
    let (mut provider, path) = sized_bundle("budget");
    let one = plan_provider(
        &mut provider,
        &view,
        request,
        PlanBudget { max_trials: 1 },
        &cancel,
    )
    .unwrap();
    let two = plan_provider(
        &mut provider,
        &view,
        request,
        PlanBudget { max_trials: 2 },
        &cancel,
    )
    .unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(one.opened.is_empty());
    assert_eq!(two.opened, planned.opened);
}

fn bundle_opened(
    provider: &mut LocalBundle,
    view: &ViewState,
    cancel: &AtomicBool,
) -> BTreeSet<String> {
    plan_provider(provider, view, request, PlanBudget::BUNDLE, cancel)
        .unwrap()
        .opened
}

#[test]
fn a_sibling_never_decides_whether_a_box_opens_bundle() {
    let (mut provider, path) = sized_bundle("siblings");
    let cancel = AtomicBool::new(false);
    let view = ViewState::default();
    let baseline = bundle_opened(&mut provider, &view, &cancel);
    assert_eq!(
        baseline.iter().map(String::as_str).collect::<Vec<_>>(),
        ["scope:mid", "scope:small"]
    );
    for sibling in ["scope:small", "scope:mid", "scope:wide"] {
        let others: BTreeSet<String> = baseline
            .iter()
            .filter(|id| *id != sibling)
            .cloned()
            .collect();
        let mut expanded = view.clone();
        expanded.set_expanded(sibling, true);
        let planned = plan_provider(
            &mut provider,
            &expanded,
            request,
            PlanBudget::BUNDLE,
            &cancel,
        )
        .unwrap();
        assert_eq!(planned.opened, others, "expanding {sibling}");
        assert!(
            planned
                .result
                .graph
                .nodes
                .iter()
                .any(|node| node.parent.as_deref() == Some(sibling))
        );
        let mut collapsed = view.clone();
        collapsed.set_expanded(sibling, false);
        assert_eq!(
            bundle_opened(&mut provider, &collapsed, &cancel),
            others,
            "collapsing {sibling}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_sibling_never_decides_whether_a_box_opens_data_messaging() {
    let cancel = AtomicBool::new(false);
    let mut provider = bundle();
    let view = ViewState::default();
    let baseline = bundle_opened(&mut provider, &view, &cancel);
    assert_eq!(
        baseline.iter().map(String::as_str).collect::<Vec<_>>(),
        ["datastore:orders"]
    );
    // Opening or closing any other box the root shows leaves the datastore open.
    let base = project_provider(&mut provider, &request(&view), &cancel).unwrap();
    for node in base.graph.nodes.iter().filter(|node| {
        node.parent.as_deref() == Some(base.graph.root.as_str()) && node.id != "datastore:orders"
    }) {
        for expanded in [true, false] {
            let mut changed = view.clone();
            changed.set_expanded(&node.id, expanded);
            assert!(
                bundle_opened(&mut provider, &changed, &cancel).contains("datastore:orders"),
                "{} expanded={expanded}",
                node.id
            );
        }
    }
}

#[test]
fn the_visible_object_limit_is_the_one_thing_that_can_keep_a_quiet_box_closed() {
    // Six quiet scopes of eight services. Opening all of them needs 54 objects.
    let limit = 30;
    // Snapshot: the limit is a diagnostic, so the batch is cut until it fits.
    let model = system(&[8; 6], 0);
    let mut view = logical();
    view.visible_budget = limit;
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(planned.opened.len(), 2, "{:?}", planned.opened);
    assert!(planned.result.nodes.len() <= limit);
    // The ones that stay closed are the later candidates, in candidate order.
    assert!(planned.opened.contains("scope:s0") && planned.opened.contains("scope:s1"));
    // With the default limit of 250 all six open.
    assert_eq!(
        planned_set(&model, &logical()).len(),
        6,
        "no limit is reached"
    );
    // Bundle: the projector leaves a region collapsed and reports it pending.
    let names = ["a", "b", "c", "d", "e", "f"].map(|name| (name, 8));
    let (mut provider, path) = scopes_bundle("limit", &names);
    let cancel = AtomicBool::new(false);
    let limited = |view: &ViewState| ProviderViewRequest {
        visible_budget: limit,
        ..request(view)
    };
    let planned =
        plan_provider(&mut provider, &view, limited, PlanBudget::BUNDLE, &cancel).unwrap();
    let graph = &planned.result.graph;
    assert!(graph.nodes.len() <= limit);
    assert!(!planned.opened.is_empty() && planned.opened.len() < 6);
    for id in &planned.opened {
        assert!(open(id, graph), "{id} was reported opened but is closed");
    }
    // Deterministic, and the same without the limit: all six open.
    let again = plan_provider(&mut provider, &view, limited, PlanBudget::BUNDLE, &cancel).unwrap();
    assert_eq!(again.opened, planned.opened);
    let all = plan_provider(
        &mut provider,
        &logical(),
        request,
        PlanBudget::BUNDLE,
        &cancel,
    )
    .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(all.opened.len(), 6);
}
