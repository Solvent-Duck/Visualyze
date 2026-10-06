use std::{path::PathBuf, sync::atomic::AtomicBool};
use visualyze_layout::{
    detail::{PlanBudget, plan_provider, plan_snapshot},
    noise::{NOISE_THRESHOLD, visual_noise},
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

#[test]
fn opening_every_scope_would_be_noisy_so_the_smallest_open_first() {
    // 31 services in five scopes: opening all of them puts 36 boxes in one row.
    let model = system(&[10, 2, 3, 12, 4], 0);
    let view = logical();
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:s1", "scope:s2", "scope:s4"]
    );
    assert!(visual_noise(&planned.result).total <= NOISE_THRESHOLD);
    assert!(open("scope:s1", &planned.result) && !open("scope:s0", &planned.result));
    // The projection is exactly the one the user would get by opening those.
    let mut by_hand = view.clone();
    for id in &planned.opened {
        by_hand.set_expanded(id, true);
    }
    assert_eq!(
        project(&model, &by_hand).unwrap().nodes.len(),
        planned.result.nodes.len()
    );
}

#[test]
fn a_view_already_over_the_threshold_opens_nothing() {
    let model = system(&[2, 2], 20);
    let view = logical();
    let base = project(&model, &view).unwrap();
    assert!(visual_noise(&base).total > NOISE_THRESHOLD);
    let planned = plan_snapshot(&model, &view, PlanBudget::SNAPSHOT).unwrap();
    assert!(planned.opened.is_empty());
    assert_eq!(planned.result.key, base.key);
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
    let planned = plan_snapshot(&model, &logical(), PlanBudget::SNAPSHOT).unwrap();
    assert_eq!(
        planned
            .opened
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["scope:inner", "scope:outer"]
    );
    assert!(planned.result.nodes.iter().any(|n| n.id == "service:a"));
    // One trial per pass: a budget of one opens only the shallow scope.
    let one = plan_snapshot(&model, &logical(), PlanBudget { max_trials: 1 }).unwrap();
    assert_eq!(
        one.opened.iter().map(String::as_str).collect::<Vec<_>>(),
        ["scope:outer"]
    );
    assert!(
        plan_snapshot(&model, &logical(), PlanBudget { max_trials: 0 })
            .unwrap()
            .opened
            .is_empty()
    );
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
    // The supplied datastore holds tables and a table holds a column; those are
    // the only boxes that hide supplied structure at the root.
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
        ["datastore:orders", "table:orders"]
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
    assert_eq!(
        parent_of("column:customer-id").as_deref(),
        Some("table:orders")
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
    for (scope, count) in [("small", 3), ("mid", 6), ("wide", 30)] {
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
    // The trial limit bounds the work: one trial opens only the first container.
    let (mut provider, path) = sized_bundle("budget");
    let one = plan_provider(
        &mut provider,
        &view,
        request,
        PlanBudget { max_trials: 1 },
        &cancel,
    )
    .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        one.opened.iter().map(String::as_str).collect::<Vec<_>>(),
        ["scope:small"]
    );
}
