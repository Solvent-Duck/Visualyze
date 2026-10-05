use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

use visualyze_layout::{
    Algorithm, LayoutRequest, QualityProfile,
    projection::LayoutPolicy,
    provider_projection::{ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::provider::{
    Basis, Coverage, EntityKind, Limits, LocalBundle, ModelProvider, PageRequest, RelationKind,
};

fn fixture() -> LocalBundle {
    LocalBundle::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/distributed-ai.bundle.jsonl"),
        Limits::default(),
    )
    .unwrap()
}

#[test]
fn distributed_fixture_keeps_messages_instances_and_coverage_explicit() {
    let mut provider = fixture();
    assert_eq!(provider.manifest().coverage, Coverage::Partial);
    assert!(
        provider
            .manifest()
            .omissions
            .iter()
            .any(|s| s.contains("synthetic"))
    );
    assert_eq!(provider.stats().counts.entities, 346);
    assert_eq!(provider.stats().counts.relations, 564);
    assert_eq!(
        provider.child_count(1, "datastore:conversations").unwrap(),
        80
    );
    let messages = provider
        .incident(
            1,
            "channel:jobs",
            PageRequest {
                limit: 128,
                after: None,
            },
        )
        .unwrap();
    assert!(messages.next.is_none());
    assert_eq!(
        messages
            .items
            .iter()
            .filter(|r| r.kind == RelationKind::Subscribes)
            .count(),
        12
    );
    assert_eq!(
        messages
            .items
            .iter()
            .filter(|r| r.kind == RelationKind::Publishes)
            .count(),
        2
    );
    assert!(
        !messages
            .items
            .iter()
            .any(|r| r.kind == RelationKind::Requests)
    );
    assert!(
        messages
            .items
            .iter()
            .all(|r| r.claim.basis == Basis::Declared)
    );
    for region in 0..3 {
        let instance = format!("instance:region-{region:02}-worker-000");
        assert_eq!(
            provider.summary(1, &instance).unwrap().unwrap().kind,
            EntityKind::DeploymentInstance
        );
        let relationships = provider
            .incident(1, &instance, PageRequest::default())
            .unwrap();
        assert_eq!(relationships.items.len(), 2);
        assert!(
            relationships
                .items
                .iter()
                .any(|r| r.kind == RelationKind::InstanceOf
                    && r.source == instance
                    && r.target == "service:worker-000")
        );
        assert!(
            relationships
                .items
                .iter()
                .any(|r| r.kind == RelationKind::DeployedIn
                    && r.target == format!("deployment:region-{region:02}-accelerators"))
        );
        // Deployment location must not silently become logical ownership.
        assert!(!relationships.items.iter().any(|r| r.kind.is_ownership()));
    }
    for i in 0..12 {
        let worker = format!("service:worker-{i:03}");
        let incident = provider
            .incident(1, &worker, PageRequest::default())
            .unwrap();
        assert!(
            incident
                .items
                .iter()
                .any(|r| r.kind == RelationKind::DependsOn
                    && r.source == worker
                    && r.target == format!("service:worker-{:03}", (i + 1) % 12))
        );
    }
    for i in 0..80 {
        let table = format!("table:conversation-{i:05}");
        let incident = provider
            .incident(1, &table, PageRequest::default())
            .unwrap();
        assert!(
            incident
                .items
                .iter()
                .any(|r| r.kind == RelationKind::TableReferences
                    && r.source == table
                    && r.target == format!("table:conversation-{:05}", (i + 1) % 80))
        );
        assert_eq!(provider.child_count(1, &table).unwrap(), 2);
    }
}

#[test]
fn distributed_regions_route_all_projected_claims_with_real_semantic_membership() {
    let mut provider = fixture();
    let cancel = AtomicBool::new(false);
    let pins = BTreeMap::new();
    for focus in [
        "scope:edge",
        "scope:inference",
        "scope:retrieval",
        "scope:events",
        "scope:learning",
        "table:conversation-00000",
    ] {
        let projection = project_provider(
            &mut provider,
            &ProviderViewRequest {
                focus: Some(focus.into()),
                policy: LayoutPolicy::Peers,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
        assert!(
            projection.unrepresented_relation_ids.is_empty(),
            "{focus} has loaded invisible claims"
        );
        let solution = solve(LayoutRequest {
            projection: &projection.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &cancel,
        })
        .unwrap_or_else(|error| panic!("{focus}: {error}"));
        validate(&projection.graph, &solution, &pins, 2).unwrap();
        assert_eq!(solution.routes.len(), projection.graph.edges.len());
        assert!(solution.boxes.len() <= 250);
        for route in solution.routes {
            assert!(!route.semantic_ids.is_empty());
            for id in route.semantic_ids {
                let original = &projection.relations[&id];
                assert_eq!(original.claim.basis, Basis::Declared);
                assert!(provider.summary(1, &original.source).unwrap().is_some());
                assert!(provider.summary(1, &original.target).unwrap().is_some());
            }
        }
    }
}
