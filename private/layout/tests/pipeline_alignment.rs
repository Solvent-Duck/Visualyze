//! A flattened pipeline (as a teaching view lays one out) stacks its chain on one
//! column, even when a second callee shares the bottom layer.
use std::sync::atomic::AtomicBool;
use visualyze_layout::{
    Algorithm, QualityProfile,
    composed::{CanonicalPins, ComposedRequest, solve_composed},
    projection::{LayoutPolicy, ViewState, project},
};
use visualyze_model::{
    architecture::Architecture,
    walkthrough::{TeachingLayout, TeachingView, teaching_model},
};

#[test]
fn teaching_pipeline_with_a_side_callee_keeps_its_column() {
    let full =
        Architecture::from_json(include_bytes!("../../../public/examples/otel-demo.json")).unwrap();
    let members = [
        "service:frontend-proxy",
        "service:chatbot",
        "service:agent",
        "service:mcp",
        "service:llm",
    ];
    let view = TeachingView {
        id: "agent-path".into(),
        title: "Agent path".into(),
        members: members.iter().map(|m| (*m).into()).collect(),
        layout: TeachingLayout::Pipeline,
    };
    let flat = teaching_model(&full, &view).unwrap();
    let mut state = ViewState::default();
    state.policy = LayoutPolicy::Pipeline;
    let projection = project(&flat, &state).unwrap();
    let solved = solve_composed(ComposedRequest {
        projection: &projection,
        algorithm: Algorithm::Floorplan,
        quality: QualityProfile::for_policy(LayoutPolicy::Pipeline),
        pins: &CanonicalPins::new(),
        work_budget: 192,
        cancelled: &AtomicBool::new(false),
    })
    .unwrap();
    let boxes = &solved
        .assembly
        .frame(&projection.root)
        .unwrap()
        .solution()
        .boxes;
    let at = |id: &str| boxes[id].bounds.center();
    let column = at("service:agent");
    for id in ["service:frontend-proxy", "service:chatbot", "service:mcp"] {
        assert_eq!(
            (at(id).x, at(id).z),
            (column.x, column.z),
            "{id} leaves the column"
        );
    }
}
