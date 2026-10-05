use std::{collections::BTreeMap, sync::atomic::AtomicBool};

use visualyze_layout::{
    Algorithm, LayoutRequest, LayoutSolution, Point, QualityProfile,
    projection::{ViewState, project},
    solve, validate,
};
use visualyze_model::architecture::Architecture;

fn notes() -> Architecture {
    Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json")).unwrap()
}

fn layout(view: &ViewState, pins: &BTreeMap<String, Point>) -> LayoutSolution {
    let projection = project(&notes(), view).unwrap();
    let solution = solve(LayoutRequest {
        projection: &projection,
        algorithm: Algorithm::Floorplan,
        pins,
        previous: None,
        quality: QualityProfile::default(),
        work_budget: 32,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    validate(&projection, &solution, pins, 2).unwrap();
    solution
}

fn route_membership(solution: &LayoutSolution) -> Vec<String> {
    let mut ids: Vec<String> = solution
        .routes
        .iter()
        .flat_map(|route| route.semantic_ids.iter().cloned())
        .collect();
    ids.sort();
    ids
}

#[test]
fn compact_and_expanded_interface_views_route_every_visible_relationship() {
    let mut view = ViewState::default();
    let compact = layout(&view, &BTreeMap::new());
    view.set_expanded("service:notes", true);
    let expanded = layout(&view, &BTreeMap::new());
    assert!(expanded.boxes.len() > compact.boxes.len());
    let owner = &expanded.boxes["service:notes"];
    let interface = &expanded.boxes["interface:notes"];
    assert!(owner.solid);
    assert_eq!(owner.bounds.max.z, interface.bounds.min.z);
    // Expanding splits the aggregated request connector into its members: the
    // visible line count grows, but every relation stays routed exactly once.
    assert!(expanded.routes.len() > compact.routes.len());
    assert_eq!(route_membership(&compact), route_membership(&expanded));
}

#[test]
fn entering_a_component_routes_boundary_references_outside_its_solid_box() {
    let mut view = ViewState::default();
    view.enter("service:notes");
    let solution = layout(&view, &BTreeMap::new());
    assert!(solution.boxes["service:notes"].solid);
    assert!(
        solution
            .boxes
            .values()
            .any(|object| object.id.starts_with("view:reference:") && object.parent.is_none())
    );
    assert!(
        solution
            .routes
            .iter()
            .any(|route| route.edge_id == "rel:read-database")
    );
}

#[test]
fn inferred_group_entry_preserves_every_supplied_relationship_as_route_or_internal_record() {
    let mut view = ViewState::with_budgets(3, 2, 250);
    let projection = project(&notes(), &view).unwrap();
    layout(&view, &BTreeMap::new());
    view.remember_groups(&projection);
    let group = projection.suggested_groups[0].id.clone();
    view.enter(&group);
    let focused = project(&notes(), &view).unwrap();
    let solution = layout(&view, &BTreeMap::new());
    assert_eq!(
        route_membership(&solution).len() + focused.internal_relation_ids.len(),
        focused
            .relation_metadata
            .values()
            .filter(|relation| !matches!(
                relation.kind,
                visualyze_model::architecture::RelationKind::Contains
                    | visualyze_model::architecture::RelationKind::Exposes
            ))
            .count()
    );
}

#[test]
fn explicit_component_pin_is_respected_after_band_alignment() {
    let view = ViewState::default();
    let original = layout(&view, &BTreeMap::new());
    let point = original.boxes["service:notes"].bounds.center();
    let pinned = layout(&view, &BTreeMap::from([("service:notes".into(), point)]));
    assert_eq!(pinned.boxes["service:notes"].bounds.center(), point);
}
