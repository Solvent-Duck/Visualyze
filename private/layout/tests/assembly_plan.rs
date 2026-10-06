use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

use visualyze_layout::{
    Algorithm, Edge, LayoutError, LayoutRequest, Node, NodeKind, Point, Projection, QualityProfile,
    assembly_plan::{FRAME_SIZING_POLICY_VERSION, FramePlan, MAX_FRAME_STEPS, plan_frames},
    projection::{ViewState, project},
    provider_projection::{ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::{
    architecture::Architecture,
    provider::{Limits, LocalBundle},
};

fn node(id: &str, parent: Option<&str>, kind: NodeKind, solid: bool) -> Node {
    Node {
        id: id.into(),
        semantic_ids: vec![format!("semantic:{id}")],
        parent: parent.map(str::to_owned),
        kind,
        band: 1,
        min_size: Point::new(12, 4, 10),
        max_size: Point::new(48, 24, 40),
        preferred_size: Point::new(24, 12, 20),
        solid,
    }
}

fn edge(id: &str, source: &str, target: &str) -> Edge {
    Edge {
        bundle: None,
        id: id.into(),
        source: source.into(),
        target: target.into(),
        semantic_ids: vec![format!("claim:{id}"), format!("claim:{id}:parallel")],
    }
}

fn nested() -> Projection {
    Projection {
        key: "original-projection-key".into(),
        model_id: "frame-plan-fixture".into(),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            node("root", None, NodeKind::Scope, false),
            node("auth", Some("root"), NodeKind::Concern, true),
            node("db", Some("root"), NodeKind::Datastore, false),
            node("table", Some("db"), NodeKind::Group, false),
            node("row", Some("table"), NodeKind::Service, true),
            node("service", Some("root"), NodeKind::Service, true),
            node("api", Some("service"), NodeKind::Interface, true),
        ],
        edges: vec![],
        relation_metadata: BTreeMap::new(),
        internal_relation_ids: vec!["collapsed-claim".into()],
        semantic_to_display: BTreeMap::new(),
        suggested_groups: vec![],
        references: BTreeMap::new(),
        diagnostics: vec!["original semantic-view diagnostic".into()],
    }
}

fn run(projection: &Projection) -> FramePlan {
    plan_frames(projection, &AtomicBool::new(false)).unwrap()
}

fn local<'a>(plan: &'a FramePlan, frame: &str, id: &str) -> &'a Node {
    plan.frames[frame]
        .projection
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap()
}

fn assert_invalid(projection: &Projection, expected: &str) {
    let error = plan_frames(projection, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, LayoutError::Invalid(_)), "{error}");
    assert!(error.to_string().contains(expected), "{error}");
}

/// This checks ordinary per-frame LCA routing only, not composed boundary paths.
fn assert_kernel_compatibility(plan: &FramePlan) {
    let cancelled = AtomicBool::new(false);
    let pins = BTreeMap::new();
    for frame in plan.frames.values() {
        for algorithm in [
            Algorithm::Floorplan,
            Algorithm::Layered,
            Algorithm::Constrained,
            Algorithm::Treemap,
        ] {
            let solution = solve(LayoutRequest {
                projection: &frame.projection,
                algorithm,
                pins: &pins,
                previous: None,
                quality: QualityProfile::default(),
                work_budget: 48,
                cancelled: &cancelled,
            })
            .unwrap_or_else(|error| {
                panic!("{:?} frame {}: {error}", algorithm, frame.projection.root)
            });
            validate(&frame.projection, &solution, &pins, 2).unwrap();
            assert_eq!(solution.routes.len(), frame.projection.edges.len());
        }
    }
}

#[test]
fn descendants_have_independent_frames_and_bounded_parent_proxies() {
    let mut projection = nested();
    let plan = run(&projection);
    assert_eq!(plan.frames.len(), 3);
    assert_eq!(plan.frames["root"].child_frames, ["db"]);
    assert_eq!(plan.frames["db"].child_frames, ["table"]);
    assert_eq!(plan.frames["table"].parent_frame.as_deref(), Some("db"));
    for (host, id) in [("root", "db"), ("db", "table")] {
        let original = projection.nodes.iter().find(|n| n.id == id).unwrap();
        let proxy = local(&plan, host, id);
        assert!(proxy.solid);
        assert_eq!(proxy.parent, original.parent);
        assert_eq!(proxy.min_size, original.min_size);
        assert_eq!(proxy.max_size, original.max_size);
        assert_eq!(proxy.preferred_size, original.preferred_size);
        assert_eq!(proxy.semantic_ids, original.semantic_ids);
        assert_eq!(proxy.kind, original.kind);
        assert_eq!(proxy.band, original.band);
        let interior = local(&plan, id, id);
        assert!(!interior.solid);
        assert!(interior.parent.is_none());
    }
    projection.nodes.extend((0..100).map(|i| {
        node(
            &format!("column:{i}"),
            Some("table"),
            NodeKind::Service,
            true,
        )
    }));
    projection.key = "a-different-full-projection".into();
    let larger = run(&projection);
    assert_eq!(
        plan.frames["root"].cache_key,
        larger.frames["root"].cache_key
    );
    assert_eq!(plan.frames["db"].cache_key, larger.frames["db"].cache_key);
    assert_ne!(
        plan.frames["table"].cache_key,
        larger.frames["table"].cache_key
    );
    assert_eq!(projection.key, "a-different-full-projection");
}

#[test]
fn solid_owners_and_interface_pads_stay_in_one_frame() {
    let mut projection = nested();
    projection.edges.push(edge("exposes", "service", "api"));
    let plan = run(&projection);
    assert!(!plan.frames.contains_key("service"));
    assert!(!plan.frames.contains_key("api"));
    assert!(local(&plan, "root", "service").solid);
    assert_eq!(
        local(&plan, "root", "api").parent.as_deref(),
        Some("service")
    );
    assert_eq!(plan.node_frames["api"], "root");
    assert_eq!(plan.edges["exposes"].lca_frame, "root");
    assert_eq!(plan.frames["root"].projection.edges, projection.edges);
    assert_kernel_compatibility(&plan);
}

#[test]
fn cross_frame_edges_keep_direction_membership_cycles_and_parallel_identity() {
    let mut projection = nested();
    projection.edges = vec![
        edge("auth-row", "auth", "row"),
        edge("auth-row-2", "auth", "row"),
        edge("row-auth", "row", "auth"),
    ];
    let plan = run(&projection);
    assert_eq!(plan.edges.len(), 3);
    let source_to_root = &plan.edges["row-auth"].source;
    assert_eq!(source_to_root.frame_id, "table");
    assert_eq!(source_to_root.representative, "db");
    assert_eq!(source_to_root.steps.len(), 2);
    assert_eq!(source_to_root.steps[0].frame_id, "table");
    assert_eq!(source_to_root.steps[0].parent_frame, "db");
    assert_eq!(source_to_root.steps[1].frame_id, "db");
    assert_eq!(source_to_root.steps[1].parent_frame, "root");
    for original in &projection.edges {
        let topology = &plan.edges[&original.id];
        assert_eq!(topology.original, *original);
        assert_eq!(topology.lca_frame, "root");
        let lifted = plan.frames["root"]
            .projection
            .edges
            .iter()
            .find(|edge| edge.id == original.id)
            .unwrap();
        assert_eq!(lifted.semantic_ids, original.semantic_ids);
        assert_eq!(lifted.source, topology.source.representative);
        assert_eq!(lifted.target, topology.target.representative);
    }
    assert!(plan.frames["db"].projection.edges.is_empty());
    assert!(plan.frames["table"].projection.edges.is_empty());
}

#[test]
fn assembly_surface_endpoints_and_ancestor_edges_need_no_fake_interior_edges() {
    let mut projection = nested();
    projection.edges = vec![
        edge("outside-db", "service", "db"),
        edge("table-outside", "table", "service"),
        edge("db-row", "db", "row"),
        edge("row-db", "row", "db"),
        edge("root-row", "root", "row"),
    ];
    let plan = run(&projection);
    let outside = &plan.edges["outside-db"];
    assert!(outside.target.surface_endpoint);
    assert_eq!(outside.target.node_id, "db");
    assert_eq!(outside.target.frame_id, "db");
    assert_eq!(outside.target.steps.len(), 1);
    assert_eq!(outside.lca_frame, "root");
    assert_eq!(plan.edges["table-outside"].source.steps.len(), 2);
    for id in ["db-row", "row-db"] {
        assert_eq!(plan.edges[id].lca_frame, "db");
    }
    assert!(plan.edges["db-row"].source.surface_endpoint);
    assert!(plan.edges["db-row"].source.steps.is_empty());
    assert_eq!(plan.edges["db-row"].target.representative, "table");
    assert_eq!(plan.frames["db"].projection.edges.len(), 2);
    assert!(plan.frames["table"].projection.edges.is_empty());
    assert!(plan.edges["root-row"].source.steps.is_empty());
    assert!(plan.edges["root-row"].source.surface_endpoint);
    assert_eq!(plan.edges["root-row"].source.representative, "root");
    assert_eq!(
        plan.frames
            .values()
            .map(|f| f.projection.edges.len())
            .sum::<usize>(),
        projection.edges.len()
    );
}

#[test]
fn different_child_assemblies_lift_to_their_immediate_common_parent() {
    let mut projection = nested();
    projection.nodes.extend([
        node("other-table", Some("db"), NodeKind::Group, false),
        node("other-row", Some("other-table"), NodeKind::Service, true),
    ]);
    projection
        .edges
        .push(edge("foreign-key", "row", "other-row"));
    let plan = run(&projection);
    let topology = &plan.edges["foreign-key"];
    assert_eq!(topology.lca_frame, "db");
    assert_eq!(topology.source.representative, "table");
    assert_eq!(topology.target.representative, "other-table");
    assert_eq!(topology.source.steps.len(), 1);
    assert_eq!(topology.target.steps.len(), 1);
    assert!(plan.frames["root"].projection.edges.is_empty());
}

#[test]
fn actual_data_table_expansion_uses_bounded_database_and_table_proxies() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../public/examples/data-messaging.bundle.jsonl");
    let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
    let projection = project_provider(
        &mut provider,
        &ProviderViewRequest {
            expanded: BTreeSet::from(["datastore:orders".into(), "table:orders".into()]),
            ..ProviderViewRequest::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap()
    .graph;
    let plan = run(&projection);
    assert!(plan.frames.contains_key("datastore:orders"));
    assert!(plan.frames.contains_key("table:orders"));
    assert!(local(&plan, &projection.root, "datastore:orders").solid);
    assert!(local(&plan, "datastore:orders", "table:orders").solid);
    assert!(!local(&plan, "table:orders", "table:orders").solid);
    for edge in &projection.edges {
        assert_eq!(plan.edges[&edge.id].original, *edge);
    }
    assert_kernel_compatibility(&plan);
}

#[test]
fn actual_solid_focus_with_interfaces_and_outside_references_preserves_forest() {
    let model = Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
        .unwrap();
    let mut view = ViewState::default();
    view.focus = Some("service:notes".into());
    let projection = project(&model, &view).unwrap();
    assert!(
        projection
            .nodes
            .iter()
            .find(|n| n.id == projection.root)
            .unwrap()
            .solid
    );
    assert!(
        projection
            .nodes
            .iter()
            .any(|n| n.id != projection.root && n.parent.is_none())
    );
    assert!(
        projection
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Interface)
    );
    let plan = run(&projection);
    assert_eq!(plan.frames.len(), 1);
    for node in &projection.nodes {
        let kernel_node = local(&plan, &projection.root, &node.id);
        assert_eq!(kernel_node.parent, node.parent);
        assert_eq!(kernel_node.solid, node.solid);
        assert_eq!(kernel_node.semantic_ids, node.semantic_ids);
        assert_eq!(plan.node_frames[&node.id], projection.root);
    }
    for edge in &projection.edges {
        assert_eq!(plan.edges[&edge.id].original, *edge);
        assert_eq!(plan.edges[&edge.id].lca_frame, projection.root);
    }
    assert_kernel_compatibility(&plan);
}

#[test]
fn parentless_expanded_regions_are_hosted_without_new_containment_claims() {
    let mut projection = nested();
    projection.nodes.extend([
        node("outside", None, NodeKind::Scope, false),
        node("outside-service", Some("outside"), NodeKind::Service, true),
    ]);
    projection
        .edges
        .push(edge("outside-call", "outside-service", "row"));
    let plan = run(&projection);
    assert_eq!(plan.frames["outside"].parent_frame.as_deref(), Some("root"));
    assert!(local(&plan, "root", "outside").parent.is_none());
    assert!(local(&plan, "outside", "outside").parent.is_none());
    assert_eq!(plan.edges["outside-call"].lca_frame, "root");
    assert_eq!(plan.edges["outside-call"].source.representative, "outside");
}

#[test]
fn reordering_source_nodes_and_edges_produces_the_same_complete_plan() {
    let mut projection = nested();
    projection.edges = vec![edge("a", "auth", "row"), edge("b", "row", "db")];
    let before = format!("{:#?}", run(&projection));
    projection.nodes.reverse();
    projection.edges.reverse();
    assert_eq!(before, format!("{:#?}", run(&projection)));
}

#[test]
fn exact_cache_domains_include_all_size_coordinates_and_policy_identity() {
    let projection = nested();
    let baseline = run(&projection);
    let changes: [fn(&mut Node); 9] = [
        |n| n.min_size.x -= 1,
        |n| n.min_size.y -= 1,
        |n| n.min_size.z -= 1,
        |n| n.max_size.x += 1,
        |n| n.max_size.y += 1,
        |n| n.max_size.z += 1,
        |n| n.preferred_size.x += 1,
        |n| n.preferred_size.y += 1,
        |n| n.preferred_size.z += 1,
    ];
    for change in changes {
        let mut changed = projection.clone();
        change(changed.nodes.iter_mut().find(|n| n.id == "db").unwrap());
        assert_eq!(changed.key, projection.key);
        let next = run(&changed);
        for frame in ["root", "db"] {
            assert_ne!(
                baseline.frames[frame].cache_key,
                next.frames[frame].cache_key
            );
            assert_ne!(
                baseline.frames[frame].projection.key,
                next.frames[frame].projection.key
            );
        }
    }
    let mut key = baseline.frames["root"].cache_key.clone();
    assert_eq!(key.sizing_policy_version, FRAME_SIZING_POLICY_VERSION);
    let old_lookup = key.lookup_key();
    key.sizing_policy_version += 1;
    assert_ne!(key.lookup_key(), old_lookup);
}

#[test]
fn kernel_projections_do_not_present_global_inspection_metadata_as_local_facts() {
    let projection = nested();
    let plan = run(&projection);
    assert_eq!(projection.internal_relation_ids, ["collapsed-claim"]);
    assert_eq!(
        projection.diagnostics,
        ["original semantic-view diagnostic"]
    );
    for frame in plan.frames.values() {
        assert!(frame.projection.internal_relation_ids.is_empty());
        assert!(frame.projection.semantic_to_display.is_empty());
        assert!(frame.projection.relation_metadata.is_empty());
        assert!(frame.projection.references.is_empty());
        assert!(frame.projection.suggested_groups.is_empty());
        assert!(frame.projection.diagnostics.is_empty());
    }
}

#[test]
fn malformed_hierarchies_and_dimensions_fail_without_a_partial_plan() {
    type Mutation = fn(&mut Projection);
    let mutations: Vec<(&str, Mutation)> = vec![
        ("1–1024", |p| p.nodes.clear()),
        ("duplicate", |p| p.nodes.push(p.nodes[0].clone())),
        ("missing frame projection root", |p| {
            p.root = "absent".into()
        }),
        ("root has a parent", |p| {
            p.nodes[0].parent = Some("db".into())
        }),
        ("missing parent", |p| {
            p.nodes[4].parent = Some("absent".into())
        }),
        ("cyclic", |p| p.nodes[2].parent = Some("table".into())),
        ("unsupported nesting", |p| p.nodes[2].solid = true),
        ("unsupported nesting", |p| {
            p.nodes[4].parent = Some("service".into())
        }),
        ("unsupported nesting", |p| {
            p.nodes
                .push(node("nested-api", Some("api"), NodeKind::Interface, true));
        }),
        ("invalid frame size", |p| p.nodes[4].min_size.x = 0),
        ("invalid frame size", |p| p.nodes[4].preferred_size.x = 100),
        ("invalid frame size", |p| p.nodes[4].band = 65),
    ];
    for (expected, mutate) in mutations {
        let mut projection = nested();
        mutate(&mut projection);
        assert_invalid(&projection, expected);
    }
}

#[test]
fn duplicate_missing_and_self_edges_are_rejected_not_silently_dropped() {
    for edges in [
        vec![edge("same", "auth", "row"), edge("same", "row", "auth")],
        vec![edge("missing", "absent", "row")],
        vec![edge("self", "row", "row")],
    ] {
        let mut projection = nested();
        projection.edges = edges;
        assert_invalid(&projection, "invalid edge");
    }
}

#[test]
fn empty_or_duplicate_memberships_are_not_accepted_as_meaningful_objects_or_routes() {
    for members in [
        vec![],
        vec!["".into()],
        vec!["claim".into(), "claim".into()],
    ] {
        let mut projection = nested();
        projection.nodes[4].semantic_ids = members.clone();
        assert_invalid(&projection, "semantic membership");
        let mut projection = nested();
        let mut route = edge("e", "auth", "row");
        route.semantic_ids = members;
        projection.edges.push(route);
        assert_invalid(&projection, "semantic membership");
    }
}

#[test]
fn node_edge_and_identity_limits_are_checked_before_expansion() {
    let mut projection = nested();
    projection.nodes.extend(
        (0..1024).map(|i| node(&format!("extra:{i}"), Some("root"), NodeKind::Service, true)),
    );
    assert_invalid(&projection, "1–1024");
    let mut projection = nested();
    projection.edges = (0..4097)
        .map(|i| edge(&format!("e:{i}"), "auth", "row"))
        .collect();
    assert_invalid(&projection, "4096");
    let mut projection = nested();
    projection.model_id = "x".repeat(8 * 1024 * 1024);
    assert_invalid(&projection, "identity data exceeds");
}

#[test]
fn route_step_limit_bounds_deep_parallel_topology() {
    let mut projection = nested();
    projection.nodes.truncate(2);
    let mut parent = "root".to_owned();
    for i in 0..64 {
        let id = format!("f:{i}");
        projection
            .nodes
            .push(node(&id, Some(&parent), NodeKind::Scope, false));
        parent = id;
    }
    projection
        .nodes
        .push(node("deep", Some(&parent), NodeKind::Service, true));
    projection.edges = (0..=MAX_FRAME_STEPS / 64)
        .map(|i| edge(&format!("e:{i}"), "deep", "auth"))
        .collect();
    assert_invalid(&projection, "boundary steps");
}

#[test]
fn pre_cancelled_extraction_returns_cancelled_even_before_input_validation() {
    let mut projection = nested();
    projection.nodes.clear();
    assert!(matches!(
        plan_frames(&projection, &AtomicBool::new(true)),
        Err(LayoutError::Cancelled)
    ));
}
